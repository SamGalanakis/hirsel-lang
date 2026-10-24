"""Ordinary Ordered launch of complete entry; retained immutable tape replay."""
import argparse,json,os,subprocess,resource,hashlib
from pathlib import Path
p=argparse.ArgumentParser();p.add_argument('--hirsel',required=True);p.add_argument('--manifest',type=Path,required=True);a=p.parse_args()
root=a.manifest.parent
rows=json.loads((root/'cases.json').read_text()); receipts=[];tapes=[];failures=[]
env={k:v for k,v in os.environ.items() if not k.startswith('HIRSEL_RT_')}
env.update(HIRSEL_JOBS='4',HIRSEL_MEMORY_LIMIT='2G',HIRSEL_DAEMON='off')
def run(argv,d,name):
    (d/(name+'.argv.json')).write_text(json.dumps(list(map(str,argv))))
    inputs=[p for p in root.glob('*.hir')]+[p for p in d.rglob('*.json')]+[Path(argv[0]).resolve()]
    (d/(name+'.inputs.json')).write_text(json.dumps({str(p):hashlib.sha256(p.read_bytes()).hexdigest() for p in inputs if p.is_file()},sort_keys=True))
    out=subprocess.run(argv,capture_output=True,env=env,timeout=300 if "build" in argv else 60,preexec_fn=None if "build" in argv else lambda: resource.setrlimit(resource.RLIMIT_CPU,(60,61)))
    (d/(name+'.stdout')).write_bytes(out.stdout);(d/(name+'.stderr')).write_bytes(out.stderr)
    return out
binaries={}
for backend in ['llvm','cranelift']:
    binary=root/backend
    if os.environ.get('RPC_GATEWAY_30_BINARIES'):
        import shutil
        shutil.copyfile(Path(os.environ['RPC_GATEWAY_30_BINARIES'])/backend,binary);binary.chmod(0o755);binaries[backend]=binary;continue
    out=run([a.hirsel,'build',str(a.manifest),'--backend',backend,'-o',str(binary)],root,'build-'+backend)
    assert out.returncode==0,(backend,out.stderr)
    binaries[backend]=binary
for backend,binary in binaries.items():
    for row in rows:
        origins=[Path(p) for p in os.environ.get('RPC_GATEWAY_30_RECOVER','').split(os.pathsep) if p]
        previous=None
        for recovered in origins:
            prior=json.loads((recovered/'receipts.json').read_text()) if (recovered/'receipts.json').exists() else []
            good=next((r for r in prior if r['id']==row['id'] and r['engine']==backend and r['passed']),None)
            if good and row.get('group')!='admission':previous=(recovered,good);break
        if previous:
            recovered,good=previous
            cell=recovered/(backend+'-'+row['id']);tape=cell/'run.tape'
            for target,executable in binaries.items():
                suffix='replay-'+target
                if (cell/(suffix+'.stderr')).exists():
                    if (cell/(suffix+'.stderr')).read_bytes(): failures.append(dict(id=row['id'],recorded=backend,replay=target,disposition='original failure retained'))
                    continue
                replay=run([str(executable),'--hirsel-replay',str(tape)],cell,suffix)
                if (replay.returncode,replay.stdout,replay.stderr)!=(good['actual']['exit'],good['actual']['stdout'].encode(),b''):
                    failures.append(dict(id=row['id'],recorded=backend,replay=target,status=replay.returncode,stderr=replay.stderr.decode()))
            tapes.append(dict(path=str(tape),expected=good['actual'],steps=row.get('steps',4000000)))
            continue
        d=root/(backend+'-'+row['id']);d.mkdir();config=d/'config';config.mkdir()
        (config/'routes.json').write_text(json.dumps(row['manifest'],separators=(',',':')))
        io=dict(stdout='stdout.bytes@1',tasks='tasks.cooperative@1',random='random.splitmix64@1',monotonic_clock='clock.monotonic.ns@1',dirs=[dict(path='config',host=str(config),access='read')])
        if 'net' in row:io['net']=row['net']
        grant=d/'grants.json';grant.write_text(json.dumps(dict(args=row['args'],io=io,placement=dict(cores=1,commit='shared',reorder='off'))))
        tape=d/'run.tape';out=run([str(binary),'--hirsel-record',str(tape),'--hirsel-grants',str(grant),*row['args']],d,'record')
        actual=dict(exit=out.returncode,stdout=out.stdout.decode());ok=actual==row['expected'] and out.stderr==b''
        if row.get('group')=='admission':
            # Output/replay bytes cannot prove pure reserve/drop transitions.
            # Keep the semantic verdict and occupancy coverage independent.
            from admission import judge_recording
            verdict=judge_recording(None,actual)
            ok=verdict['semantic_verdict']=='pass' and out.stderr==b''
        else:
            verdict=dict(semantic_verdict='pass' if ok else 'fail',coverage=None,recorded_output=actual)
        receipts.append(dict(id=row['id'],engine=backend,actual=actual,passed=ok,**verdict,replay_verdict={}))
        (root/'receipts.json').write_text(json.dumps(receipts,indent=2))
        if not ok:
            failures.append(dict(id=row['id'],recorded=backend,semantic_verdict=verdict['semantic_verdict']))
        (config/'routes.json').unlink()
        for target,executable in binaries.items():
            replay=run([str(executable),'--hirsel-replay',str(tape)],d,'replay-'+target)
            identical=(replay.returncode,replay.stdout,replay.stderr)==(out.returncode,out.stdout,out.stderr)
            receipts[-1]['replay_verdict'][target]='pass' if identical else 'fail'
            if not identical:failures.append(dict(id=row['id'],recorded=backend,replay=target,status=replay.returncode,stderr=replay.stderr.decode()))
        (root/'receipts.json').write_text(json.dumps(receipts,indent=2))
        tapes.append(dict(path=str(tape),expected=actual,steps=row.get('steps',4000000)))
print(json.dumps(dict(completed=len(receipts),tapes=tapes,failures=failures)))
