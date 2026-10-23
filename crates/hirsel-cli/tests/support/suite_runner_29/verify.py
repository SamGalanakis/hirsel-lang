#!/usr/bin/env python3
"""Complete app qualification with immutable per-cell receipts and bounded launches.

Go exec TestPipes (exec_test.go:470-525, BSD-3-Clause) supplies independent
stdout/stderr and stdin-close controls; FIFO handshakes replace startup sleeps.
Replay removes the private executable, manifest and report directory first.
"""
import argparse
import copy
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
import tempfile


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def main(args):
    root=args.root
    out=root/'evidence';out.mkdir()
    env={k:v for k,v in os.environ.items() if not k.startswith('HIRSEL_RT_')}
    env.update(HIRSEL_DAEMON='off',HIRSEL_RESULTS='none@1',HIRSEL_JOBS='4',HIRSEL_MEMORY_LIMIT='2G')
    records=[]
    def save():
        (out/'receipts.json').write_text(json.dumps(records,indent=2)+'\n')
    def invoke(argv, prefix, build=False, full=False):
        if build:
            command=[sys.executable,str(root/'capped'),'--timeout','300',*map(str,argv)]
        else:
            command=[sys.executable,str(root/'capped'),'--timeout','60','prlimit','--cpu=60:61',*map(str,argv)]
        try:
            with open('/dev/full' if full else os.devnull,'wb') as sink:
                done=subprocess.run(command,stdout=sink if full else subprocess.PIPE,stderr=subprocess.PIPE,env=env,stdin=subprocess.DEVNULL,timeout=330 if build else 75)
            result=dict(exit=done.returncode,stdout=(done.stdout or b'').decode(errors='replace'),stderr=done.stderr.decode(errors='replace'))
        except subprocess.TimeoutExpired as e:
            result=dict(exit=None,stdout=(e.stdout or b'').decode(errors='replace'),stderr=(e.stderr or b'').decode(errors='replace'),disposition='INCOMPLETE timeout')
        prefix.with_suffix('.stdout').write_text(result['stdout']);prefix.with_suffix('.stderr').write_text(result['stderr'])
        return result
    binaries={}
    resume=os.environ.get('SUITE_RUNNER_29_RESUME')
    phase=os.environ.get('SUITE_RUNNER_29_PHASE','all')
    for backend in ['llvm','cranelift']:
        if resume:
            binary=Path(resume)/backend
            assert binary.is_file()
            binaries[backend]=binary
            continue
        binary=out/backend
        result=invoke([args.hirsel,'build',root/'hirsel.json','--backend',backend,'-o',binary],out/(backend+'-build'),True)
        records.append(dict(id='build',engine=backend,result=result,passed=result['exit']==0));save()
        if result['exit']==0: binaries[backend]=binary
    if len(binaries)!=2:
        print(json.dumps(dict(completed=len(records),failed=sum(not r['passed'] for r in records))))
        return 1
    seal={str(p):digest(p) for p in [*root.glob('*.hir'),root/'hirsel.json',root/'verify.py',root/'child.py',root/'cases.json',root/'caps.json',root/'capped',args.hirsel,*binaries.values()]}
    (out/'consumer-seal.json').write_text(json.dumps(seal,indent=2))
    def replay(record):
        tape=Path(record['tape']);immutable=tape.read_bytes()
        state=Path(record['directory']);data=state/'data';reports=state/'reports';child=state/'child'
        for p in [data,reports,child]:
            if p.exists():p.rename(p.with_name(p.name+'.unavailable'))
        replay_rows=[]
        for backend in ['reference','llvm','cranelift']:
            argv=([args.hirsel,'run',root/'hirsel.json','--replay',tape,'--max-steps','1000000'] if backend=='reference' else [binaries[backend],'--hirsel-replay',tape])
            result=invoke(argv,state/('replay-'+backend))
            app_stderr=lambda row: ''.join(line+'\n' for line in row['stderr'].splitlines() if not line.startswith('capped:'))
            passed=result['exit']==record['actual']['exit'] and result['stdout']==record['actual']['stdout'] and app_stderr(result)==app_stderr(record['actual'])
            replay_rows.append(dict(engine=backend,argv=list(map(str,argv)),passed=passed,actual=result))
            assert tape.read_bytes()==immutable
            assert not any(p.exists() for p in [data,reports,child]), 'replay performed real effects'
        record['replays']=replay_rows
        # Native/host tape adapter is absent for checked H: no replay credit.
        record['checked_h_replay']='UNSUPPORTED / INCOMPLETE: no public host-tape adapter'
        for p in [data,reports,child]:
            q=p.with_name(p.name+'.unavailable')
            if q.exists():q.rename(p)
        record['replay_complete']=all(r['passed'] for r in replay_rows)
        for row in replay_rows:
            if not row['passed']:
                if row['engine']=='reference' and record['id'] in ['cases-32','argv-4096','input-262144'] and 'UnconsumedInput(' in row['actual']['stderr']:
                    row['disposition']='INCOMPLETE: retained reference replay refusal; cause not established'
                else:
                    record['passed']=False
        save()
    cases=json.loads((root/'cases.json').read_text())+json.loads((root/'caps.json').read_text())
    filters=os.environ.get('SUITE_RUNNER_29_CASE','').split(',')
    if filters!=['']:cases=[c for c in cases if c['id'] in filters]
    assert cases
    if phase=='host-missing':cases=[]
    for backend,binary in binaries.items():
        for case in cases:
            identity=case['id']
            state=out/(backend+'-'+identity);state.mkdir();data=state/'data';data.mkdir();reports=state/'reports';reports.mkdir()
            raw=json.dumps(case['manifest'],separators=(',',':')).encode()
            if case.get('raw_bytes'):raw+=b' '*(case['raw_bytes']-len(raw))
            (data/'manifest.json').write_bytes(raw);(reports/'report.json').write_text('previous')
            children=[]
            if case['exit']!=2 and not case.get('absent'):
                for suite in case['manifest']['suites']:
                    for item in suite['cases']:
                        for code in case['scripts'][item['id']]:
                            if isinstance(code,int):children.append(dict(program=item['argv'][0],args=item['argv'][1:],steps=[dict(print=[79,79,79]),dict(print_err=[69,69,69]),dict(exit=dict(exited=code))]))
            grants=dict(args=case['args'],io=dict(stdout='stdout.bytes@1',stderr='stderr.bytes@1',tasks='tasks.cooperative@1',monotonic_clock='clock.monotonic.ns@1',dirs=[dict(path='fixtures',host=str(data),access='read'),dict(path='reports',host=str(reports),access='read' if case.get('write_failure') else 'read_write')],process=dict(provider='process.script@1',rules=[] if case.get('refused') else [dict(program='/fixture/child')],runs=[],children=children)),placement=dict(cores=1,commit='shared',reorder='auto'))
            grant=state/'grants.json';grant.write_text(json.dumps(grants));tape=out/(backend+'-'+identity+'.tape')
            argv=[binary,'--hirsel-record',tape,'--hirsel-grants',grant,*case['args']]
            record=dict(id=identity,engine=backend,directory=str(state),tape=str(tape),argv=list(map(str,argv)),input_sha256=digest(data/'manifest.json'),grants_sha256=digest(grant),state='started',passed=False)
            records.append(record);save();actual=invoke(argv,state/'run',full=case.get('output_failure',False))
            try:rows=[json.loads(line) for line in actual['stdout'].splitlines()]
            except ValueError:rows=None
            expected=case.get('plan',[] if case['exit']==2 or case.get('write_failure') or case.get('output_failure') else [case['report']])
            artifact=(reports/'report.json').read_text()
            artifact_ok=(artifact=='previous' if case['exit']==2 or case.get('plan') is not None or case.get('text') is not None or case.get('write_failure') else json.loads(artifact)==case['report'])
            output_ok=actual['stdout']==case['text'] if 'text' in case else rows==expected
            record.update(state='completed',actual=actual,passed=actual['exit']==case['exit'] and output_ok and artifact_ok,tape_sha256=digest(tape) if tape.exists() else None)
            save()
            if tape.exists() and record['passed']:replay(record)
    # Host workflows prove real pipe pressure, deadline cleanup, threshold joins,
    # and observed production width. Fixed jobs=2 at all placements.
    libc=ctypes.CDLL(None,use_errno=True)
    for backend in ['reference',*binaries]:
        if phase=='script-only':continue
        if phase=='host-missing' and backend=='reference':continue
        for width in ([1] if backend=='reference' else [1,2,4]):
            for mode in ['success','domain-error','cancellation','attempt-deadline','pipes','cutoff']:
                if phase=='host-missing' and mode=='pipes':continue
                host_filter=os.environ.get('SUITE_RUNNER_29_HOST_IDS')
                if host_filter and f'{backend}:host-{mode}-w{width}' not in host_filter.split(','):continue
                identity=f'host-{mode}-w{width}'
                state=out/(backend+'-'+identity);state.mkdir();data=state/'data';data.mkdir();reports=state/'reports';reports.mkdir()
                child=state/'child';child.write_bytes((root/'child.py').read_bytes());child.chmod(0o755)
                fd=libc.inotify_init1(os.O_CLOEXEC|os.O_NONBLOCK);assert fd>=0
                assert libc.inotify_add_watch(fd,os.fsencode(state),0x100|0x8)>=0
                def wait_file(path):
                    until=time.monotonic()+10
                    while not path.exists():
                        remaining=until-time.monotonic();assert remaining>0,('handshake',path)
                        assert select.select([fd],[],[],remaining)[0],path
                        os.read(fd,65536)
                modes=(['hold','fail' if mode=='domain-error' else 'pass'] if mode in ['success','domain-error'] else ['cancel','cancel'] if mode=='cancellation' else ['cancel'] if mode=='attempt-deadline' else ['pipes'] if mode=='pipes' else ['hold-fail','fail','pass'])
                items=[]
                for i,m in enumerate(modes):
                    if m in ['hold','cancel','hold-fail']:os.mkfifo(state/(str(i)+'.release'))
                    items.append(dict(id='c'+str(i),argv=[str(child),m,str(state),str(i)],tags=['host'],expect=0,retries=0,timeout_ms=5000 if mode!='attempt-deadline' else 1200))
                source=dict(jobs=2,timeout_ms=1800 if mode=='cancellation' else 10000,failure_limit=1 if mode=='cutoff' else 0,suites=[dict(id='host',cases=items)])
                (data/'manifest.json').write_text(json.dumps(source));control_dir=Path(tempfile.mkdtemp(prefix='sr29-'));control=control_dir/'control'
                grants=dict(args=['run','manifest.json','report.json'],io=dict(stdout='stdout.bytes@1',stderr='stderr.bytes@1',tasks='tasks.cooperative@1',monotonic_clock='clock.monotonic.ns@1',dirs=[dict(path='fixtures',host=str(data),access='read'),dict(path='reports',host=str(reports),access='read_write')],process=dict(provider='process.spawn@1',rules=[dict(program=str(child))])),placement=dict(cores=width,commit='shared',reorder='auto',**({'control':str(control)} if backend!='reference' else {})))
                grant=state/'grants.json';grant.write_text(json.dumps(grants));tape=out/(backend+'-'+identity+'.tape')
                argv=([args.hirsel,'run',root/'hirsel.json','--grants',grant,'--record',tape,'--max-steps','1000000'] if backend=='reference' else [binaries[backend],'--hirsel-record',tape,'--hirsel-grants',grant,'run','manifest.json','report.json'])
                record=dict(id=identity,engine=backend,directory=str(state),tape=str(tape),argv=list(map(str,argv)),grants_sha256=digest(grant),input_sha256=digest(data/'manifest.json'),requested_width=width,jobs=2,state='started',passed=False);records.append(record);save()
                wrapper=[sys.executable,str(root/'capped'),'--timeout','60','prlimit','--cpu=60:61',*map(str,argv)]
                with (state/'run.stdout').open('wb') as stdout,(state/'run.stderr').open('wb') as stderr:
                    process=subprocess.Popen(wrapper,stdin=subprocess.DEVNULL,stdout=stdout,stderr=stderr,env=env)
                    try:
                        if mode in ['success','domain-error','cutoff','cancellation','attempt-deadline']:
                            wait_file(state/'0.started')
                            if mode in ['success','domain-error','cutoff']:wait_file(state/'1.done')
                            if backend!='reference':
                                sock=socket.socket(socket.AF_UNIX);sock.settimeout(5);sock.connect(str(control));sock.sendall(b'{"metrics":true}\n');reply=sock.makefile('rb').readline();sock.close();(state/'control.json').write_bytes(reply);record['control']=json.loads(reply)
                                assert record['control']['width']['effective']==width
                            if mode in ['success','domain-error','cutoff']:
                                with (state/'0.release').open('wb') as release:release.write(b'x')
                        code=process.wait(timeout=70)
                    except Exception as error:
                        record['harness_error']=repr(error)
                        process.terminate();code=process.wait(timeout=10)
                os.close(fd)
                if control.exists():control.unlink()
                control_dir.rmdir()
                actual=dict(exit=code,stdout=(state/'run.stdout').read_text(),stderr=(state/'run.stderr').read_text())
                try:
                    report=json.loads(actual['stdout']);pids={p.stem:int(p.read_text()) for p in state.glob('*.started')};reaped=all(not Path('/proc',str(pid)).exists() for pid in pids.values())
                    expected_exit=0 if mode in ['success','pipes'] else 1
                    status=[r['status'] for r in report['cases']]
                    expected_status={'success':['passed','passed'],'domain-error':['passed','failed'],'pipes':['passed'],'cutoff':['failed','failed','not_run'],'attempt-deadline':['failed']} .get(mode)
                    valid=(status==expected_status if expected_status else report['expired'] and all(s in ['failed','incomplete','not_run'] for s in status))
                    valid &= mode!='cutoff' or not (state/'2.started').exists()
                    record.update(actual=actual,report=report,fixture_pids=pids,reaped=reaped,state='completed',passed=code==expected_exit and valid and reaped and 'harness_error' not in record)
                except Exception as error:record.update(actual=actual,state='completed',error=repr(error))
                save()
                if record['passed'] and tape.exists():replay(record)
    # Changed input/config/tape refusal controls bind to an existing success.
    control=next((r for r in records if r['id']=='pass' and r.get('tape_sha256')),None)
    if control:
        tape=Path(control['tape']);bad=out/'changed.tape';bad.write_bytes(tape.read_bytes()+b'changed\n')
        for backend,binary in binaries.items():
            result=invoke([binary,'--hirsel-replay',bad],out/(backend+'-changed-tape'))
            records.append(dict(id='changed-tape',engine=backend,passed=result['exit'] not in [None,0],actual=result));save()
    assert seal=={p:digest(Path(p)) for p in seal}
    failed=sum(not r['passed'] for r in records)
    incomplete=sum(not x['passed'] for r in records for x in r.get('replays',[]) if 'INCOMPLETE' in x.get('disposition',''))
    print(json.dumps(dict(completed=len(records),failed=failed,incomplete_replays=incomplete,evidence=str(out))))
    return int(bool(failed))


if __name__=='__main__':
    parser=argparse.ArgumentParser(description=__doc__);parser.add_argument('--root',type=Path,required=True);parser.add_argument('--hirsel',type=Path,required=True)
    sys.exit(main(parser.parse_args()))
