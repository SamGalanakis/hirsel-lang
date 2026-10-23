#!/usr/bin/env python3
"""Ordinary Ordered launch, full-app native records, immutable cross-engine replays."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import resource
import subprocess
import sys
from disposition import known


def main(a):
    root=a.root;out=root/'evidence';out.mkdir()
    env={k:v for k,v in os.environ.items() if not k.startswith('HIRSEL_RT_')}
    env.update(HIRSEL_DAEMON='off',HIRSEL_RESULTS='none@1',HIRSEL_JOBS='4',HIRSEL_MEMORY_LIMIT='2G')
    cases=json.loads((root/'cases.json').read_text())
    if os.environ.get('DAGBUILD_29_CAPS'):
        from caps import boundary_cases
        cases=boundary_cases()
    if os.environ.get('DAGBUILD_29_REAL_ONLY'):cases=[]
    if os.environ.get('DAGBUILD_29_CASE'):cases=[c for c in cases if c['id'] in os.environ['DAGBUILD_29_CASE'].split(',')]
    assert cases or os.environ.get('DAGBUILD_29_REAL_ONLY')
    twins=json.loads((root/'reference-cases.json').read_text())
    full_ids={row['full_id'] for row in twins}
    selected_ids={case['id'] for case in cases}
    cases += [row['case'] for row in twins if row['full_id'] in selected_ids]
    receipts=[]
    skip=set()
    for path in os.environ.get('DAGBUILD_29_RESUME','').split(','):
        if path:
            for row in json.loads(Path(path).read_text()):
                if row.get('state')=='completed':skip.add(row['id'])
    (out/'resume-identities.json').write_text(json.dumps(sorted(skip)))
    def run(argv,label,setup=False):
        def limits():
            resource.setrlimit(resource.RLIMIT_CPU,(60,61))
            resource.setrlimit(resource.RLIMIT_AS,(2*1024**3,2*1024**3))
        wrapped=argv if setup else [sys.executable,str(root/'capped'),'--timeout','60',*argv]
        try:
            p=subprocess.run(wrapped,env=env,stdin=subprocess.DEVNULL,capture_output=True,timeout=300 if setup else 75,preexec_fn=None if setup else limits)
        except subprocess.TimeoutExpired as e:
            p=subprocess.CompletedProcess(wrapped,124,e.stdout or b'',e.stderr or b'')
        (out/(label+'.stdout')).write_bytes(p.stdout);(out/(label+'.stderr')).write_bytes(p.stderr)
        return p
    binaries={}
    for backend in ['llvm','cranelift']:
        binary=Path(os.environ['DAGBUILD_29_BINARY_DIR'])/backend if os.environ.get('DAGBUILD_29_BINARY_DIR') else out/backend
        if os.environ.get('DAGBUILD_29_BINARY_DIR'):
            assert binary.is_file()
            binaries[backend]=binary
            continue
        p=run([str(a.hirsel),'build',str(root/'hirsel.json'),'--backend',backend,'-o',str(binary)],backend+'-build',True)
        assert p.returncode==0,(backend,p)
        binaries[backend]=binary
    for backend,binary in binaries.items():
        if os.environ.get('DAGBUILD_29_BACKEND') and backend!=os.environ['DAGBUILD_29_BACKEND']:continue
        for case in cases:
            for width in ([1,2,4] if case['id'] in ['diamond-first','failure-descendants'] else [1]):
                identity=f'{backend}-{case["id"]}-w{width}'
                if identity in skip:continue
                d=out/identity;d.mkdir();project=d/'project';state=d/'state';project.mkdir();state.mkdir()
                if case['denied']!='missing':
                    source=json.dumps(case['manifest'])
                    if case.get('raw_size'):source+=' '*(case['raw_size']-len(source.encode()))
                    (project/'manifest.json').write_text(source)
                for name,data in case['files'].items():(project/name).write_text(data)
                if case['prior'] is not None:(state/'state.json').write_text(case['prior'] if isinstance(case['prior'],str) else json.dumps(case['prior']))
                children=[dict(program=t['argv'][0],args=t['argv'][1:],steps=[dict(print=[79,79]),dict(print_err=[69,69]),dict(exit=dict(exited=7 if t['argv'][1]=='fail' else 0))]) for t in case['manifest']['tasks']]
                grants=dict(args=case['args'],io=dict(stdout='stdout.bytes@1',stderr='stderr.bytes@1',tasks='tasks.cooperative@1',monotonic_clock='clock.monotonic.ns@1',dirs=[dict(path='project',host=str(project),access='read_write'),dict(path='state',host=str(state),access='read' if case['denied']=='write' else 'read_write')],process=dict(provider='process.script@1',rules=[] if case['denied']=='spawn' else [dict(program='/fixture/child')],runs=[],children=children)),placement=dict(cores=width,commit='shared',reorder='auto'))
                if case['denied']=='read':grants['io']['dirs']=[m for m in grants['io']['dirs'] if m['path']!='project']
                g=d/'grants.json';g.write_text(json.dumps(grants));tape=d/'record.tape'
                argv=[str(binary),'--hirsel-record',str(tape),'--hirsel-grants',str(g),*case['args']]
                receipt=dict(id=identity,argv=argv,state='started',requested_width=width,logical_jobs=2,provider='scripted process + real file/clock',physical_width='unobserved',grants_sha256=hashlib.sha256(g.read_bytes()).hexdigest())
                receipts.append(receipt);(out/'receipts.json').write_text(json.dumps(receipts,indent=2))
                p=run(argv,identity)
                stderr=''.join(l+'\n' for l in p.stderr.decode().splitlines() if not l.startswith('capped:'))
                actual=dict(exit=p.returncode,rows=[json.loads(l) for l in p.stdout.splitlines()],stderr=stderr)
                expected={k:case[k] for k in ['exit','rows','stderr']}
                receipt.update(state='completed',actual=actual,passed=actual==expected)
                if p.returncode==124:
                    assert identity=='llvm-diamond-first-w4',('unexpected live timeout',identity,p)
                    receipt['disposition']='INCOMPLETE: e1-live-hang-29; original 60s cap'
                    receipt['replays']=[dict(engine=e,passed=False,disposition='INCOMPLETE: live tape has no complete trailer') for e in ['reference','llvm','cranelift']]
                    (out/'receipts.json').write_text(json.dumps(receipts,indent=2))
                    continue
                if case['state'] is not None:
                    assert json.loads((state/'state.json').read_text())==case['state'],identity
                assert actual==expected,(identity,actual,expected)
                original=tape.read_bytes();receipt['tape_sha256']=hashlib.sha256(original).hexdigest()
                project.rename(d/'project-unavailable');state.rename(d/'state-unavailable')
                # Replay has no mounted original data or executable provider. Full
                # consumption is enforced by each launcher's complete-tape adapter.
                receipt['replays']=[]
                # Full native inputs and exact output/state assertions stay above.
                # Reference replay consumes a separately recorded, bounded input twin.
                replay_engines=['llvm','cranelift'] if case['id'] in full_ids else ['reference','llvm','cranelift']
                for replay_engine in replay_engines:
                    replay_argv=(['env','HIRSEL_JOBS=1',str(a.hirsel),'run',str(root/'hirsel.json'),'--replay',str(tape),'--max-steps','1000000'] if replay_engine=='reference' else [str(binaries[replay_engine]),'--hirsel-replay',str(tape)])
                    q=run(replay_argv,identity+'-replay-'+replay_engine)
                    qstderr=''.join(l+'\n' for l in q.stderr.decode().splitlines() if not l.startswith('capped:'))
                    passed=q.returncode==p.returncode and q.stdout==p.stdout and qstderr==stderr
                    receipt['replays'].append(dict(engine=replay_engine,exit=q.returncode,passed=passed))
                    (out/'receipts.json').write_text(json.dumps(receipts,indent=2))
                    if not passed:
                        assert known('script',backend,case['id'],width,replay_engine,q),(identity,replay_engine,q)
                        receipt['replays'][-1]['disposition']='INCOMPLETE: exact retained replay refusal; see known-incomplete.json'
                        (out/'receipts.json').write_text(json.dumps(receipts,indent=2))
                    assert not project.exists() and not state.exists()
                    assert tape.read_bytes()==original
                (out/'receipts.json').write_text(json.dumps(receipts,indent=2))
    real=[]
    if not os.environ.get('DAGBUILD_29_CAPS') and not os.environ.get('DAGBUILD_29_SKIP_REAL'):
        from real import qualify
        real=qualify(root,out,binaries,run,env,a.hirsel)
        if os.environ.get('DAGBUILD_29_INCREMENTAL'):
            from real import incremental
            incremental(root,out,binaries,run,a.hirsel)
    if os.environ.get('DAGBUILD_29_CONTROL_TAPE'):
        from controls import controls
        controls(root,out,binaries,run,a.hirsel)
    print(json.dumps(dict(completed=len(receipts),passed=sum(r['passed'] for r in receipts),real=len(real))))

if __name__=='__main__':
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('--root',type=Path,required=True);p.add_argument('--hirsel',type=Path,required=True)
    main(p.parse_args())
