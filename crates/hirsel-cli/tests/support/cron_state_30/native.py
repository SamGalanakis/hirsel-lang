#!/usr/bin/env python3
"""Production Ordered launches. A status-only child handshake holds readiness open."""
import argparse,datetime,hashlib,json,os,selectors,socket,subprocess,time
from pathlib import Path

def write(p,v):p.write_text(json.dumps(v,indent=2)+'\n')
def utc(ns):return datetime.datetime.fromtimestamp(ns/1e9,datetime.timezone.utc).isoformat(timespec='seconds').replace('+00:00','Z')
def main(a):
 a.hirsel=a.hirsel.resolve();a.capped=a.capped.resolve();a.manifest=a.manifest.resolve()
 root=a.root;root.mkdir(parents=True,exist_ok=True)
 env={k:v for k,v in os.environ.items() if not k.startswith('HIRSEL_RT_')}
 env.update(HIRSEL_DAEMON='off',HIRSEL_RESULTS='none@1',HIRSEL_JOBS='4',HIRSEL_MEMORY_LIMIT='2G')
 app=root/a.backend
 build=[str(a.hirsel),'build',str(a.manifest),'--backend',a.backend,'-o',str(app)]
 done=subprocess.run(build,env=env,capture_output=True,timeout=300)
 (root/'build.stdout').write_bytes(done.stdout);(root/'build.stderr').write_bytes(done.stderr)
 assert done.returncode==0,(build,done.stderr.decode())
 child=root/'child.py';child.write_text('''import socket,sys\ns=socket.socket(socket.AF_UNIX);s.connect(sys.argv[1]);s.sendall(b'ready\\n');s.recv(1)\nsys.stdout.write('O'*100000);sys.stderr.write('E'*100000)\nsys.exit(7 if sys.argv[2]=='error' else 0)\n''')
 receipts=[]
 selected=os.environ.get('CRON_STATE_30_CELL')
 assert selected is None or selected in [f'{mode}-{width}' for mode in ['success','error','cancel'] for width in [1,2,4]], 'invalid native cell'
 for mode in ['success','error','cancel']:
  for width in [1,2,4]:
   if selected is not None and selected!=f'{mode}-{width}':continue
   id=f'{a.backend}-{mode}-{width}';d=root/id;d.mkdir();data=d/'config';data.mkdir();state=d/'state';state.mkdir()
   # Unix path lengths are independent of long evidence paths.
   import tempfile
   with tempfile.TemporaryDirectory(prefix='cronctl-') as short:
    control=Path(short)/'ctl';peer=Path(short)/'peer'
    listener=socket.socket(socket.AF_UNIX);listener.bind(str(peer));listener.listen(1);listener.settimeout(10)
    config=dict(version=1,parallel=4,jobs=[dict(id='a',cron='* * * * *',program='/usr/bin/python3',argv=[str(child),str(peer),mode],deadline_ms=3000)])
    write(data/'schedule.json',config)
    until=utc(time.time_ns()+(1500000000 if mode=='cancel' else 10000000000))
    argv=['run','schedule.json',until,'1']
    grants=dict(args=argv,io=dict(stdout='stdout.bytes@1',stderr='stderr.bytes@1',tasks='tasks.cooperative@1',wall_clock='clock.wall.unix_ns@1',monotonic_clock='clock.monotonic.ns@1',dirs=[dict(path='config',host=str(data),access='read'),dict(path='state',host=str(state),access='read_write')],process=dict(provider='process.spawn@1',rules=[dict(program='/usr/bin/python3')])),placement=dict(cores=width,control=str(control),commit='shared',reorder='auto'))
    grant=d/'grants.json';write(grant,grants);tape=d/'record.tape'
    cmd=[str(a.capped),'--timeout','15','prlimit','--cpu=60:61','--as=2147483648',str(app),'--hirsel-record',str(tape),'--hirsel-grants',str(grant),*argv]
    r=dict(id=id,argv=cmd,state='started');receipts.append(r);write(root/'receipts.json',receipts)
    proc=subprocess.Popen(cmd,env=env,stdout=subprocess.PIPE,stderr=subprocess.PIPE)
    try:
     connection,_=listener.accept();assert connection.recv(100)==b'ready\n'
     # The child is running, with the parent held in child.wait. This is an
     # application handshake, not a socket-existence/startup-width inference.
     ctl=socket.socket(socket.AF_UNIX);ctl.settimeout(5);ctl.connect(str(control));ctl.sendall(b'{"metrics":true}\n')
     reply=b''
     while not reply.endswith(b'\n'):reply+=ctl.recv(65536)
     write(d/'control.json',json.loads(reply));ctl.close()
     metrics=json.loads(reply);r['width']=metrics
     r['ready']=metrics['width']['effective']==width and metrics['width']['pending']==0 and metrics.get('worker_phase')=='running'
     if mode!='cancel':connection.sendall(b'x')
     out,err=proc.communicate(timeout=15)
     connection.close();listener.close()
    except Exception as e:
     proc.kill();out,err=proc.communicate();r['setup_error']=repr(e)
    (d/'capped.stderr').write_bytes(err)
    err=b''.join(line for line in err.splitlines(keepends=True) if not line.startswith(b'capped:'))
    (d/'stdout').write_bytes(out);(d/'stderr').write_bytes(err)
    r.update(state='completed',exit=proc.returncode,stdout=out.decode(errors='replace'),stderr=err.decode(errors='replace'))
    r['passed']=proc.returncode==(0 if mode=='success' else 1) and (err==b'{"error":"run_until"}\n' if mode=='cancel' else not err) and r.get('ready',False) and not (state/'owner.lock').exists()
    r['tape_sha256']=hashlib.sha256(tape.read_bytes()).hexdigest() if tape.exists() else None
    write(root/'receipts.json',receipts)
    print(json.dumps(r),flush=True)
    assert r['passed'],r
 # Original commands and grants unavailable during every replay.
 child.rename(root/'child.unavailable')
 for r in receipts:
  d=root/r['id'];(d/'config').rename(d/'config.unavailable');(d/'state').rename(d/'state.unavailable');(d/'grants.json').rename(d/'grants.unavailable')
  for engine in ['reference','llvm','cranelift']:
   binary=root/engine
   if engine!='reference' and not binary.exists():continue
   cmd=([str(a.hirsel),'run',str(a.manifest),'--replay',str(d/'record.tape'),'--max-steps','10000000'] if engine=='reference' else [str(binary),'--hirsel-replay',str(d/'record.tape')])
   replay_env=dict(env,HIRSEL_JOBS='1') if engine=='reference' else env
   done=subprocess.run([str(a.capped),'--timeout','15','prlimit','--cpu=60:61','--as=2147483648',*cmd],env=replay_env,capture_output=True)
   (d/(engine+'-replay.stdout')).write_bytes(done.stdout);(d/(engine+'-replay.stderr')).write_bytes(done.stderr)
   assert done.returncode==r['exit'] and done.stdout==r['stdout'].encode() and b''.join(line for line in done.stderr.splitlines(keepends=True) if not line.startswith(b'capped:'))==r['stderr'].encode(),(r['id'],engine,done)
   assert hashlib.sha256((d/'record.tape').read_bytes()).hexdigest()==r['tape_sha256']
 print(json.dumps(dict(completed=len(receipts),passed=sum(r['passed'] for r in receipts))))
if __name__=='__main__':
 p=argparse.ArgumentParser();p.add_argument('--root',type=Path,required=True);p.add_argument('--hirsel',type=Path,required=True);p.add_argument('--manifest',type=Path,required=True);p.add_argument('--capped',type=Path,required=True);p.add_argument('--backend',choices=['llvm','cranelift'],required=True);main(p.parse_args())
