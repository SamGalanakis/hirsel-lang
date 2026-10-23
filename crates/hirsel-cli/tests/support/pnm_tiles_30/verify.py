"""Production binary and immutable host-free replay driver (no runtime adapter)."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys


def octets(value):
    if isinstance(value, list): return bytes(value)
    return bytes(value['prefix']) + bytes([value['value']])*value['repeat']


def main(args):
    root = args.root
    data = json.loads((root/'cases.json').read_bytes())
    evidence = root/'native'
    evidence.mkdir()
    env = {k:v for k,v in os.environ.items() if not k.startswith('HIRSEL_RT_')}
    env.update(HIRSEL_DAEMON='off', HIRSEL_RESULTS='none@1', HIRSEL_JOBS='4', HIRSEL_MEMORY_LIMIT='2G', HIRSEL_TIMING='off')
    tools={}
    for name in ['HIRSEL_IMAGE','HIRSEL_RUNTIME_ARCHIVES','HIRSEL_LINK_KIT','LLVM_SYS_231_PREFIX']:
        value=env.get(name)
        if not value:continue
        path=Path(value).resolve()
        candidates=[path] if path.is_file() else [p for p in path.rglob('*') if p.is_file() and (p.suffix in ['.a','.so'] or p.name in ['clang','clang-23','ld.lld','llvm-ar','manifest.json'])]
        for p in candidates:
            tools[str(p)]=hashlib.sha256(p.read_bytes()).hexdigest()
    for name in [str(args.hirsel),sys.executable,shutil.which('prlimit')]:
        p=Path(name).resolve();tools[str(p)]=hashlib.sha256(p.read_bytes()).hexdigest()
    (evidence/'actual-tools-before.json').write_text(json.dumps(dict(cwd=os.getcwd(),files=tools,config={k:v for k,v in env.items() if k.startswith('HIRSEL_')}),indent=2)+'\n')
    selected = [r for r in data['cases'] if not os.getenv('PNM_TILES_30_CASE') or r['id'] in os.environ['PNM_TILES_30_CASE'].split(',')]
    assert selected
    tapes=[]; receipts=[]
    def run(identity, command, expected, setup=False):
        receipt=dict(id=identity,argv=list(map(str,command)),started=True,completed=False,passed=False,inputs={str(p):hashlib.sha256(p.read_bytes()).hexdigest() for p in [root/'hirsel.json',root/'cases.json',root/'verify.py',root/'capped',*[root/(n+'.hir') for n in ['main','image','pnm','filter','tiles','report']]]})
        receipts.append(receipt)
        (evidence/'receipts.json').write_text(json.dumps(receipts,indent=2)+'\n')
        print('PNM_TILES_30_STARTED '+identity,flush=True)
        wrapped=[sys.executable,root/'capped','--timeout','300' if setup else '60']
        if not setup:wrapped+=['prlimit','--cpu=60:61']
        actual=subprocess.run(list(map(str,wrapped+command)),env=env,capture_output=True,timeout=330 if setup else 90)
        (evidence/(identity+'.stdout')).write_bytes(actual.stdout)
        (evidence/(identity+'.resources')).write_bytes(actual.stderr)
        # tools/capped adds its own diagnostics to stderr; child captures separately.
        receipt.update(completed=True,status=actual.returncode)
        if setup:
            assert actual.returncode==0,(identity,actual)
        else:
            result=json.loads((evidence/(identity+'.child.json')).read_bytes())
            assert (result['status'],result['stdout'],result['stderr'])==(expected['exit'],expected['stdout'],expected['stderr']),(identity,result,expected)
            assert actual.returncode==expected['exit'],(identity,actual)
        receipt['passed']=True
        (evidence/'receipts.json').write_text(json.dumps(receipts,indent=2)+'\n')
        print('PNM_TILES_30_PASS '+identity,flush=True)
    def app(identity,command,row):
        run(identity,[sys.executable,root/'verify.py','--child',evidence/(identity+'.child.json'),'--',*command],row)
    for backend in ['llvm','cranelift']:
        run('build-'+backend,[args.hirsel,'build',root/'hirsel.json','--backend',backend,'-o',evidence/backend],None,True)
        for row in selected:
            if row.get('pending') or row.get('fault'):
                # Actual Pending regular reads are covered by public SimulationConfig.
                # Production host files have no fixture readiness override; retained separately.
                receipts.append(dict(id=backend+'-'+row['id'],engine=backend,started=False,completed=False,passed=False,disposition=('INCOMPLETE: full-entry native Pending boundary witnessed externally' if row.get('pending') else 'INCOMPLETE UNRUN: no full-entry fault binding attempted; no unsupported-engine claim')))
                (evidence/'receipts.json').write_text(json.dumps(receipts,indent=2)+'\n')
                continue
            id=row['id']; name=backend+'-'+id
            mount=root/('data-'+name);mount.mkdir()
            (mount/id).write_bytes(octets(data['files'][id])); (mount/'output').write_bytes(b'preserve')
            grants=dict(args=row['args'],io=dict(stdout='stdout.bytes@1',stderr='stderr.bytes@1',tasks='tasks.cooperative@1',dirs=[dict(path='data',host=str(mount),access=row.get('access','read_write'))]),placement=dict(cores=1,commit='shared',reorder='auto'))
            grant=evidence/(name+'.grants.json'); grant.write_text(json.dumps(grants)+'\n')
            tape=evidence/(name+'.tape')
            app(name,[evidence/backend,'--hirsel-record',tape,'--hirsel-grants',grant,*row['args']],row)
            assert (mount/id).read_bytes()==octets(data['files'][id])
            assert (mount/'output').read_bytes()==(octets(row['expected']) if row['expected'] is not None else b'preserve')
            assert sorted(p.name for p in mount.iterdir())==sorted([id,'output'])
            mount.rename(root/('unavailable-'+name))
            grant.unlink()
            record=dict(row,tape=str(tape.relative_to(root)),id=name)
            tapes.append(record)
            (root/'tapes.json').write_text(json.dumps(tapes)+'\n')
    for row in tapes:
        tape=root/row['tape'];before=hashlib.sha256(tape.read_bytes()).hexdigest()
        for backend in ['llvm','cranelift']:
            app(row['id']+'-replay-'+backend,[evidence/backend,'--hirsel-replay',tape],row)
        assert hashlib.sha256(tape.read_bytes()).hexdigest()==before
    assert all(hashlib.sha256(Path(p).read_bytes()).hexdigest()==h for p,h in tools.items()),'tool changed during consumers'
    print(json.dumps(dict(recordings=len(tapes),receipts=len(receipts))))


if __name__=='__main__':
    if len(sys.argv)>1 and sys.argv[1]=='--child':
        actual=subprocess.run(sys.argv[4:],capture_output=True)
        Path(sys.argv[2]).write_text(json.dumps(dict(status=actual.returncode,stdout=list(actual.stdout),stderr=list(actual.stderr)))+'\n')
        sys.exit(actual.returncode)
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--root',type=Path,required=True)
    parser.add_argument('--hirsel',type=Path,required=True)
    main(parser.parse_args())
