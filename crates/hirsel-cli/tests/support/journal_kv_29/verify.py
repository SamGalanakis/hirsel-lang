"""Complete journalkv CLI workflows; native build once/backend, replay each tape.
The independent byte fixtures are frozen in cases.json before compiling the app.
"""
import argparse, hashlib, json, os, shutil, subprocess
from pathlib import Path


def fixture_bytes(value):
    if isinstance(value,str): return bytes.fromhex(value)
    output=bytearray.fromhex(value.get('prefix',''))
    if 'frame' in value:
        frame=bytes.fromhex(value['frame']);assert len(frame)<=8199 and value['repeat']<=257
        for _ in range(value['repeat']):
            remaining=32768-len(output)%32768
            if remaining<len(frame):output.extend(bytes(remaining))
            output.extend(frame)
    else:
        assert value['count']<=1048577
        output.extend(bytes([value['byte']])*value['count'])
    assert len(output)<=1048577
    return bytes(output)

def main(a):
    root=a.root
    env=dict(os.environ,HIRSEL_MEMORY_LIMIT='2G',HIRSEL_JOBS='4')
    for key in list(env):
        if key.startswith('HIRSEL_RT_'):env.pop(key)
    receipts=[]
    def save(): (root/'receipts.json').write_text(json.dumps(receipts,indent=2)+'\n')
    def call(argv,name,setup=False):
        limit='300' if setup else '60'
        wrapped=['python3',str(root/'capped'),'--timeout',limit]
        if not setup:wrapped+=['prlimit','--cpu=60:61']
        done=subprocess.run(wrapped+list(map(str,argv)),stdin=subprocess.DEVNULL,capture_output=True,env=env)
        (root/(name+'.stdout')).write_bytes(done.stdout)
        (root/(name+'.stderr')).write_bytes(done.stderr)
        # tools/capped's report is outside application stdout/stderr. Retain it
        # intact in the raw receipt; compare the complete application report.
        app_stderr=b''.join(line for line in done.stderr.splitlines(keepends=True) if not line.startswith(b'capped: '))
        return done.returncode,done.stdout,app_stderr
    binaries={}
    for backend in ['llvm','cranelift']:
        binary=(a.reuse or root)/('app-'+backend)
        if a.reuse:
            assert binary.is_file()
            binaries[backend]=binary
            continue
        status,out,err=call([a.hirsel,'build',root/'hirsel.json','--backend',backend,'-o',binary],backend+'-build',True)
        assert status==0,(backend,status,out,err)
        binaries[backend]=binary
    selected=json.loads((root/'cases.json').read_text())
    # Public production native file grants have no fault queue. Those exact
    # scripted cells stay unsupported here; reference/checked H execute them.
    selected=[c for c in selected if 'fault' not in c]
    if a.case:selected=[c for c in selected if c['id'] in a.case]
    assert selected
    for backend,binary in binaries.items():
        for case in selected:
            id=case['id'];name=backend+'-'+id
            mutable=root/'mutable';mutable.mkdir()
            for d in ['data','commands']:(mutable/d).mkdir()
            for path,raw in case['files'].items():(mutable/path).write_bytes(fixture_bytes(raw))
            grants=root/'grants.json'
            grant={'args':case['args'],'io':{'stdout':'stdout.bytes@1','tasks':'tasks.cooperative@1','dirs':[
              {'path':'commands','host':str(mutable/'commands'),'access':'read'},
              {'path':'data','host':str(mutable/'data'),'access':'read_write'}]},'placement':{'cores':1,'commit':'shared','reorder':'auto'}}
            grants.write_text(json.dumps(grant))
            (root/(name+'-grants.json')).write_text(grants.read_text())
            tape=root/(name+'.tape')
            argv=[binary,'--hirsel-record',tape,'--hirsel-grants',grants,*case['args']]
            receipt=dict(id=id,engine=backend,state='started',argv=list(map(str,argv)),expected_exit=case['exit'],expected_stdout=case['stdout'],expected_error=case['error'])
            receipts.append(receipt);save()
            status,out,err=call(argv,name)
            receipt.update(state='completed',exit=status,stdout=out.decode(errors='replace'),stderr=err.decode(errors='replace'));save()
            assert status==case['exit'],(name,status,out,err)
            assert out==case['stdout'].encode(),(name,out)
            if case['error'] is None:assert not err,(name,err)
            else:assert err.startswith(('error: '+case['error']+'(').encode()),(name,err)
            actual={str(p.relative_to(mutable)):p.read_bytes().hex() for p in mutable.rglob('*') if p.is_file()}
            expected={p:fixture_bytes(v).hex() for p,v in case['expected_files'].items()}
            assert actual==expected,(name,actual,case['expected_files'])
            immutable=hashlib.sha256(tape.read_bytes()).hexdigest()
            shutil.rmtree(mutable);grants.unlink()
            replays={}
            for replay_engine in ['reference','llvm','cranelift']:
                replay_argv=([a.hirsel,'run',root/'hirsel.json','--replay',tape] if replay_engine=='reference' else [binaries[replay_engine],'--hirsel-replay',tape])
                replay=call(replay_argv,name+'-replay-'+replay_engine)
                assert replay==(status,out,err),(name,replay_engine,replay,(status,out,err))
                assert not mutable.exists()
                assert hashlib.sha256(tape.read_bytes()).hexdigest()==immutable
                replays[replay_engine]='pass'
            receipt.update(passed=True,tape_sha256=immutable,replay=replays);save()
            print(json.dumps({'id':id,'engine':backend,'passed':True,'replays':3}),flush=True)
    assert all(row.get('passed') for row in receipts)
    print(json.dumps({'selected':len(receipts),'completed':len(receipts),'passed':len(receipts),'replays':len(receipts)*3}),flush=True)

if __name__=='__main__':
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--hirsel',type=Path,required=True)
    parser.add_argument('--root',type=Path,required=True)
    parser.add_argument('--case',action='append')
    parser.add_argument('--reuse',type=Path)
    main(parser.parse_args())
