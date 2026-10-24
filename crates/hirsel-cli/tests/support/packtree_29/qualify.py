"""Focused ordinary Ordered placement; immutable host roots and fixed metadata."""
import argparse
import ctypes
import hashlib
import json
import os
from pathlib import Path
import select
import socket
import subprocess
import sys
import time
from host_replay import Batch


def main(a):
    q=a.root/('qualification-reference' if a.reference_twin else 'qualification');q.mkdir()
    sys.path.insert(0,str(a.root))
    from oracle import archive,compact,report
    env={k:v for k,v in os.environ.items() if not k.startswith('HIRSEL_RT_')}
    env.update(HIRSEL_MEMORY_LIMIT='2G',HIRSEL_JOBS='4',HIRSEL_DAEMON='off',HIRSEL_RESULTS='none@1',HIRSEL_TIMING='off')
    trees=q/'trees';trees.mkdir()
    for name in ('source','archives','target'):(trees/name).mkdir()
    twins={row['full_id']:row['case'] for row in json.loads((a.root/'reference-cases.json').read_text())}
    entries=([(p,k,bytes.fromhex(b)) for p,k,b in twins['qualification-success']['entries']] if a.reference_twin else [(f'f{i}','file',bytes([65+i])*4096) for i in range(4)])
    for p,k,b in entries:(trees/'source'/p).write_bytes(b)
    decl=dict(schema=1,entries=[dict(path=p,type=k,size=len(b)) for p,k,b in entries])
    (trees/'source/tree.json').write_text(compact(decl))
    bad=(bytes.fromhex(twins['qualification-domain']['files']['archives/bad.tar']) if a.reference_twin else archive([('a','file',b'x'*4096),('a','file',b'')]))
    (trees/'archives/bad.tar').write_bytes(bad)
    receipt=[];groups={}
    libc=ctypes.CDLL(None,use_errno=True)
    for workflow in ('success','domain'):
        expected=dict(status=0,stdout=report(entries),stderr='') if workflow=='success' else dict(status=2,stdout='',stderr=compact(dict(kind='duplicate_path',path='a'))+'\n')
        argv=['pack','tree.json','out.tar','4'] if workflow=='success' else ['unpack','bad.tar']
        for backend in ('llvm','cranelift'):
            for width in (1,2,4):
                id=f'{workflow}-{backend}-w{width}'
                dest=trees/'archives/out.tar'
                if dest.exists():dest.unlink()
                for p in [*trees.rglob('*'),trees]:os.utime(p,ns=(0,0),follow_symlinks=False)
                control=q/(id+'.sock')
                grants=dict(io=dict(stdout='stdout.bytes@1',stderr='stderr.bytes@1',tasks='tasks.cooperative@1',dirs=[dict(path=n,host=str(trees/n),access='read' if n=='source' else 'read_write') for n in ('source','archives','target')]),args=argv,placement=dict(cores=width,commit='shared',reorder='auto',control=str(control)))
                gp=q/(id+'.grants.json');gp.write_text(json.dumps(grants,indent=2))
                tape=q/(id+'.tape');trace=q/(id+'.trace');actual=q/(id+'.result.json')
                fd=libc.inotify_init1(os.O_CLOEXEC|os.O_NONBLOCK);assert fd>=0
                assert libc.inotify_add_watch(fd,os.fsencode(q),0x100|0x8)>=0
                command=[sys.executable,str(a.root/'capped'),'--timeout','60','prlimit','--cpu=60:61',sys.executable,str(Path(__file__)),'--child',str(actual),'--',str(a.root/'evidence'/backend),'--hirsel-trace-events',str(trace),'--hirsel-record',str(tape),'--hirsel-grants',str(gp),*argv]
                rec=dict(id=id,argv=command,started=1,completed=0,passed=0,requested=width,snapshots=[])
                receipt.append(rec);(q/'receipts.json').write_text(json.dumps(receipt,indent=2))
                child=subprocess.Popen(command,env=env,stdout=subprocess.PIPE,stderr=subprocess.PIPE)
                deadline=time.monotonic()+10
                while child.poll() is None:
                    if control.exists():
                        try:
                            with socket.socket(socket.AF_UNIX) as connection:
                                connection.settimeout(0.5);connection.connect(str(control));connection.sendall(b'{"metrics":true}\n')
                                data=b''
                                while not data.endswith(b'\n'):
                                    part=connection.recv(65536)
                                    if not part:break
                                    data+=part
                            if data:
                                snap=json.loads(data);rec['snapshots'].append(snap)
                                if snap['width']['effective']==width:break
                        except (OSError,ValueError):pass
                    else:
                        remaining=deadline-time.monotonic()
                        if remaining<=0 or not select.select([fd],[],[],remaining)[0]:break
                        os.read(fd,65536)
                os.close(fd)
                stdout,stderr=child.communicate(timeout=80)
                (q/(id+'.resources')).write_bytes(stderr)
                rec.update(completed=int(actual.exists()))
                if actual.exists():
                    rec['actual']=json.loads(actual.read_text());rec['passed']=int(rec['actual']==expected and child.returncode==expected['status'])
                rec['effective_observed']=any(s['width']['effective']==width for s in rec['snapshots'])
                (q/'receipts.json').write_text(json.dumps(receipt,indent=2))
                assert rec['passed'],rec
                groups.setdefault(workflow,[]).append(str(tape))
                print('PACKTREE_29_QUALIFIED '+id+' physical='+str(rec['effective_observed']),flush=True)
    (q/'semantic-groups.json').write_text(json.dumps(groups,indent=2))
    # No granted host path exists during replay. Each engine consumes the tape.
    trees.rename(q/'unavailable')
    replays=[]
    host=Batch(a.root,a.root/'hirsel.json',env,'qualification-reference' if a.reference_twin else 'qualification')
    for workflow,tapes in groups.items():
        expected=dict(status=0,stdout=report(entries),stderr='') if workflow=='success' else dict(status=2,stdout='',stderr=compact(dict(kind='duplicate_path',path='a'))+'\n')
        for tape in tapes:
            original=hashlib.sha256(Path(tape).read_bytes()).hexdigest()
            for backend in (('llvm','cranelift','reference') if a.reference_twin else ('llvm','cranelift')):
                id=Path(tape).stem+'-replay-'+backend
                cmd=[str(a.root/'evidence'/backend),'--hirsel-replay',tape] if backend!='reference' else [str(a.hirsel),'run',str(a.root/'hirsel.json'),'--replay',tape,'--max-steps','1000000']
                actual=q/(id+'.result.json')
                def complete(observed,id=id,expected=expected,tape=tape,original=original):
                    rec=dict(id=id,actual=observed,expected=expected,passed=observed==expected)
                    assert rec['passed'],rec
                    replays.append(rec);(q/'replays.json').write_text(json.dumps(replays,indent=2))
                    assert not trees.exists() and hashlib.sha256(Path(tape).read_bytes()).hexdigest()==original
                if backend=='reference':
                    host.add(tape,actual,complete)
                    continue
                command=[sys.executable,str(a.root/'capped'),'--timeout','60','prlimit','--cpu=60:61',sys.executable,str(Path(__file__)),'--child',str(actual),'--',*cmd]
                done=subprocess.run(command,env=env,capture_output=True,timeout=90)
                (q/(id+'.resources')).write_bytes(done.stderr)
                complete(json.loads(actual.read_text()))
    host.flush()
    print(json.dumps(dict(qualified=len(receipt),effective=sum(r['effective_observed'] for r in receipt),replays=len(replays),replay_passes=sum(r['passed'] for r in replays))))


if __name__=='__main__':
    if len(sys.argv)>1 and sys.argv[1]=='--child':
        done=subprocess.run(sys.argv[4:],capture_output=True)
        Path(sys.argv[2]).write_text(json.dumps(dict(status=done.returncode,stdout=done.stdout.decode(),stderr=done.stderr.decode())))
        sys.exit(done.returncode)
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--reference-twin',action='store_true')
    parser.add_argument('--root',type=Path,required=True)
    parser.add_argument('--hirsel',type=Path,required=True)
    main(parser.parse_args())
