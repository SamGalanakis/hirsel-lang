#!/usr/bin/env python3
"""Real-child qualification, readiness handshakes, live width and drain receipts."""
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
from disposition import known


def qualify(root,out,binaries,run,env,hirsel):
    libc=ctypes.CDLL(None,use_errno=True)
    receipts=[]
    skip=set()
    for path in os.environ.get('DAGBUILD_29_RESUME','').split(','):
        if path:
            skip.update(r['id'] for r in json.loads(Path(path).read_text()) if r.get('state')=='passed')
    for backend,binary in binaries.items():
        if os.environ.get('DAGBUILD_29_BACKEND') and backend!=os.environ['DAGBUILD_29_BACKEND']:continue
        for workflow in ['success','domain-error','cancel-cleanup']:
            for width in [1,2,4]:
                identity=f'real-{backend}-{workflow}-w{width}'
                if identity in skip:continue
                d=out/identity;d.mkdir();project=d/'project';state=d/'state';project.mkdir();state.mkdir()
                for name in ['a','x']:os.mkfifo(project/(name+'.release'))
                fd=libc.inotify_init1(os.O_CLOEXEC|os.O_NONBLOCK)
                assert fd>=0
                assert libc.inotify_add_watch(fd,os.fsencode(project),0x100|0x8)>=0
                def wait_file(path):
                    until=time.monotonic()+15
                    while not path.exists():
                        remaining=until-time.monotonic()
                        if remaining<=0:return False
                        if not select.select([fd],[],[],remaining)[0]:return False
                        os.read(fd,65536)
                    return True
                def task(name,deps,mode):
                    return dict(name=name,argv=['/usr/bin/python3',str(root/'child.py'),mode,str(project),name],dependencies=deps,inputs=[],outputs=[])
                tasks=[task('a',[],'hold'),task('x',[],'hold'),task('b',['a'],'ok')]
                if workflow!='success':tasks.append(task('f',[],'fail'))
                manifest=dict(tasks=tasks,jobs=2,fail_fast=workflow=='cancel-cleanup')
                (project/'manifest.json').write_text(json.dumps(manifest))
                ctl=Path('/tmp')/('dagb-'+hashlib.sha256(str(d).encode()).hexdigest()[:16]+'.ctl')
                grants=dict(args=['build','manifest.json'],io=dict(stdout='stdout.bytes@1',stderr='stderr.bytes@1',tasks='tasks.cooperative@1',monotonic_clock='clock.monotonic.ns@1',dirs=[dict(path='project',host=str(project),access='read_write'),dict(path='state',host=str(state),access='read_write')],process=dict(provider='process.spawn@1',rules=[dict(program='/usr/bin/python3')])),placement=dict(cores=width,commit='shared',reorder='auto',control=str(ctl)))
                g=d/'grants.json';g.write_text(json.dumps(grants));tape=d/'record.tape'
                argv=[str(binary),'--hirsel-record',str(tape),'--hirsel-grants',str(g),'build','manifest.json']
                with (d/'stdout').open('wb') as stdout,(d/'stderr').open('wb') as stderr:
                    p=subprocess.Popen([sys.executable,str(root/'capped'),'--timeout','60','prlimit','--cpu=60:61',*argv],stdin=subprocess.DEVNULL,stdout=stdout,stderr=stderr,env=env)
                    admitted=wait_file(project/'a.started') and wait_file(project/'x.started')
                    if not admitted:
                        code=p.wait(timeout=75)
                        assert identity=='real-cranelift-cancel-cleanup-w4' and code==124,('unexpected admission/liveness failure',identity,code)
                        observed={str(path.name):int(path.read_text()) for path in project.glob('*.started')}
                        survivors=[pid for pid in observed.values() if Path('/proc',str(pid)).exists()]
                        receipts.append(dict(id=identity,exit=code,requested_width=width,effective_width=None,disposition='INCOMPLETE: live deadline timeout; effective topology unobserved; e1-live-hang-29',pids=observed,survivors=survivors))
                        (out/'real-receipts.json').write_text(json.dumps(receipts,indent=2))
                        os.close(fd)
                        continue
                    connection=socket.socket(socket.AF_UNIX);connection.settimeout(5);connection.connect(str(ctl));connection.sendall(b'{"metrics":true}\n')
                    reply=b''
                    while not reply.endswith(b'\n'):reply+=connection.recv(65536)
                    connection.close();snapshot=json.loads(reply);(d/'control.json').write_bytes(reply)
                    # Requested placement is reported independently of physical width.
                    effective=snapshot['width']['effective']
                    assert 1<=effective<=width
                    with (project/'x.release').open('wb') as gate:gate.write(b'x')
                    # x's completion opens a slot for fail; a stays live until
                    # explicit cancellation or (success/domain) release.
                    if workflow!='cancel-cleanup':
                        with (project/'a.release').open('wb') as gate:gate.write(b'x')
                    code=p.wait(timeout=75)
                os.close(fd)
                rows=[json.loads(line) for line in (d/'stdout').read_bytes().splitlines()]
                assert code==(0 if workflow=='success' else 1),(identity,code,(d/'stderr').read_text())
                names=[t['name'] for t in tasks];assert [r['name'] for r in rows[:-1]]==names
                if workflow=='success':assert all(r['status']=='built' for r in rows[:-1])
                elif workflow=='domain-error':assert rows[-2]['status']=='failed' and rows[0]['status']=='built'
                else:assert rows[0]['status']=='canceled' and rows[2]['status']=='canceled'
                pids=[int(p.read_text()) for p in project.glob('*.started')]
                for pid in pids:assert not Path('/proc',str(pid)).exists(),('child not reaped',identity,pid)
                successful={r['name'] for r in rows[:-1] if r['status'] in ['built','clean']}
                assert {r['name'] for r in json.loads((state/'state.json').read_text())['tasks']}==successful
                before=hashlib.sha256(tape.read_bytes()).hexdigest()
                project.rename(d/'project-unavailable');state.rename(d/'state-unavailable');(root/'child.py').rename(root/'child-unavailable')
                receipt=dict(id=identity,state='passed',exit=code,requested_width=width,effective_width=effective,control=snapshot,pids=pids,reaped=True,tape_sha256=before,replays=[])
                receipts.append(receipt);(out/'real-receipts.json').write_text(json.dumps(receipts,indent=2))
                for replay_engine in ['reference','llvm','cranelift']:
                    argv=(['env','HIRSEL_JOBS=1',str(hirsel),'run',str(root/'hirsel.json'),'--replay',str(tape),'--max-steps','1000000'] if replay_engine=='reference' else [str(binaries[replay_engine]),'--hirsel-replay',str(tape)])
                    q=run(argv,identity+'-replay-'+replay_engine)
                    passed=q.returncode==code and q.stdout==(d/'stdout').read_bytes()
                    receipt['replays'].append(dict(engine=replay_engine,exit=q.returncode,passed=passed))
                    (out/'real-receipts.json').write_text(json.dumps(receipts,indent=2))
                    if not passed:
                        assert known('real',backend,workflow,width,replay_engine,q),(identity,replay_engine,q)
                        receipt['replays'][-1]['disposition']='INCOMPLETE: exact retained replay refusal; reference-replay-29 or artifact replay owner'
                        (out/'real-receipts.json').write_text(json.dumps(receipts,indent=2))
                    assert not project.exists() and not state.exists() and not (root/'child.py').exists()
                    assert hashlib.sha256(tape.read_bytes()).hexdigest()==before
                (root/'child-unavailable').rename(root/'child.py')
    return receipts
def incremental(root,out,binaries,run,hirsel):
    from oracle import record
    receipts=[]
    for backend,binary in binaries.items():
        if os.environ.get('DAGBUILD_29_BACKEND') and backend!=os.environ['DAGBUILD_29_BACKEND']:continue
        d=out/('incremental-'+backend);d.mkdir();project=d/'project';state=d/'state';project.mkdir();state.mkdir()
        (project/'in').write_text('alpha')
        def copy_task(name,source,target,deps):
            return dict(name=name,argv=['/usr/bin/python3',str(root/'child.py'),'copy',str(project/source),str(project/target)],dependencies=deps,inputs=[source],outputs=[target])
        tasks=[copy_task('a','in','mid',[]),copy_task('b','mid','out',['a'])]
        grants=dict(args=['build','manifest.json'],io=dict(stdout='stdout.bytes@1',stderr='stderr.bytes@1',tasks='tasks.cooperative@1',monotonic_clock='clock.monotonic.ns@1',dirs=[dict(path='project',host=str(project),access='read_write'),dict(path='state',host=str(state),access='read_write')],process=dict(provider='process.spawn@1',rules=[dict(program='/usr/bin/python3')])),placement=dict(cores=1,commit='shared',reorder='auto'))
        g=d/'grants.json';g.write_text(json.dumps(grants))
        previous=None
        for change,statuses,reasons in [
            ('first',['built','built'],['no_record','no_record']),
            ('unchanged',['clean','clean'],['unchanged','unchanged']),
            ('input',['built','built'],['fingerprint_changed','fingerprint_changed']),
            ('command',['built','built'],['fingerprint_changed','fingerprint_changed']),
            ('missing',['clean','built'],['unchanged','output_missing']),
            ('changed-output',['clean','built'],['unchanged','output_changed']),
        ]:
            if change=='input':(project/'in').write_text('beta')
            if change=='command':tasks[0]['argv'].append('explicit-config-change')
            if change=='missing':(project/'out').unlink()
            if change=='changed-output':(project/'out').write_text('tampered')
            (project/'manifest.json').write_text(json.dumps(dict(tasks=tasks,jobs=2,fail_fast=False)))
            identity=f'incremental-{backend}-{change}';tape=d/(change+'.tape')
            p=run([str(binary),'--hirsel-record',str(tape),'--hirsel-grants',str(g),'build','manifest.json'],identity)
            assert p.returncode==0,(identity,p)
            rows=[json.loads(l) for l in p.stdout.splitlines()]
            assert [r['status'] for r in rows[:-1]]==statuses,(identity,rows)
            assert [r['reason'] for r in rows[:-1]]==reasons,(identity,rows)
            files={n:(project/n).read_text() for n in ['in','mid','out']}
            expected=record(tasks,files)
            assert json.loads((state/'state.json').read_text())==expected,(identity,expected)
            assert files['in']==files['mid']==files['out']
            receipt=dict(id=identity,exit=0,rows=rows,state_oracle=True,replays=[])
            receipts.append(receipt)
            original=tape.read_bytes()
            project.rename(d/'project-unavailable');state.rename(d/'state-unavailable');(root/'child.py').rename(root/'child-unavailable')
            for replay_engine in ['reference','llvm','cranelift']:
                argv=(['env','HIRSEL_JOBS=1',str(hirsel),'run',str(root/'hirsel.json'),'--replay',str(tape),'--max-steps','1000000'] if replay_engine=='reference' else [str(binaries[replay_engine]),'--hirsel-replay',str(tape)])
                q=run(argv,identity+'-replay-'+replay_engine)
                passed=q.returncode==0 and q.stdout==p.stdout
                receipt['replays'].append(dict(engine=replay_engine,exit=q.returncode,passed=passed))
                if not passed:
                    assert known('incremental',backend,change,1,replay_engine,q),(identity,replay_engine,q)
                    receipt['replays'][-1]['disposition']='INCOMPLETE: exact retained replay refusal; reference-replay-29 or artifact replay owner'
                assert tape.read_bytes()==original
                assert not project.exists() and not state.exists() and not (root/'child.py').exists()
                (out/'incremental-receipts.json').write_text(json.dumps(receipts,indent=2))
            (root/'child-unavailable').rename(root/'child.py');(d/'project-unavailable').rename(project);(d/'state-unavailable').rename(state)
    return receipts
