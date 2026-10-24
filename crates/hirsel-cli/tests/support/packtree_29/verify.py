"""Bounded real-provider app matrix and immutable host-free tape replay."""
import argparse
import hashlib
import io
import json
import os
from pathlib import Path
import subprocess
import sys
import tarfile
from oracle import report
from host_replay import Batch
from vectors import cases as cap_cases


def digest(p):
    return hashlib.sha256(p.read_bytes()).hexdigest()


def receipt_summary(receipts):
    # A resumed success does not erase an earlier unfinished attempt of that ID.
    unfinished=[r['id'] for r in receipts if r.get('started') and not r.get('completed') and not r.get('disposition')]
    return dict(selected=len({r['id'] for r in receipts}),attempts=len(receipts),
        started=sum(r.get('started',0) for r in receipts),
        completed=sum(r.get('completed',0) for r in receipts),
        passed=sum(r.get('passed',0) for r in receipts),
        completed_unique=len({r['id'] for r in receipts if r.get('completed')}),
        passed_unique=len({r['id'] for r in receipts if r.get('passed')}),
        unfinished_attempts=unfinished,
        incomplete_cells=len(unfinished)+sum('disposition' in r for r in receipts))


def main(args):
    root=args.root
    out=root/'evidence';out.mkdir(exist_ok=args.resume)
    env={k:v for k,v in os.environ.items() if not k.startswith('HIRSEL_RT_')}
    env.update(HIRSEL_DAEMON='off',HIRSEL_RESULTS='none@1',HIRSEL_JOBS='4',HIRSEL_MEMORY_LIMIT='2G',HIRSEL_TIMING='off')
    rows=json.loads((root/'cases.json').read_text())
    existing={r['id'] for r in rows}
    for row in cap_cases():
        if row['id'] in existing: row['id']+='-cap'
        rows.append(row)
    assert len({r['id'] for r in rows})==len(rows)
    selected=os.environ.get('PACKTREE_29_CASE')
    if selected: rows=[r for r in rows if r['id'] in selected.split(',')]
    rows=[r for r in rows if not r.get('fault') and not r.get('metadata')]
    assert rows
    twins=json.loads((root/'reference-cases.json').read_text())
    full_ids={row['full_id'] for row in twins}
    selected_ids={row['id'] for row in rows}
    rows += [row['case'] for row in twins if row['full_id'] in selected_ids]
    host = Batch(root, root / 'hirsel.json', env, 'verify')
    toolpaths=[args.hirsel,Path(env['PACKTREE_29_HOST_REPLAYER']),Path(env['HIRSEL_IMAGE']),Path(env['LLVM_SYS_231_PREFIX'])/'bin/clang',Path(env['LLVM_SYS_231_PREFIX'])/'bin/ld.lld']
    toolpaths += list(Path(env['HIRSEL_RUNTIME_ARCHIVES']).rglob('*.a'))
    toolpaths += list(Path(env['HIRSEL_LINK_KIT']).rglob('manifest.json'))
    (out/('resume-tools.json' if args.resume else 'prebuild-tools.json')).write_text(json.dumps({str(p):digest(p) for p in toolpaths},indent=2))
    binaries={}
    for backend in ('llvm','cranelift'):
        binary=out/backend
        if args.resume:
            assert binary.exists();binaries[backend]=binary;continue
        done=subprocess.run([str(args.hirsel),'build',str(root/'hirsel.json'),'--backend',backend,'-o',str(binary)],env=env,capture_output=True,timeout=300)
        (out/(backend+'-build.stdout')).write_bytes(done.stdout)
        (out/(backend+'-build.stderr')).write_bytes(done.stderr)
        assert done.returncode==0,(backend,done)
        binaries[backend]=binary
    seal={str(p):digest(p) for p in [*root.glob('*.hir'),root/'hirsel.json',root/'cases.json',root/'verify.py',root/'host_replay.py',root/'oracle.py',root/'vectors.py',args.hirsel,*binaries.values()]}
    if args.resume:
        original=json.loads((out/'inputs.json').read_text())
        for p,h in original.items():
            path=Path(p)
            if not path.is_absolute():path=Path(os.environ['PACKTREE_29_ORIGINAL_CWD'])/path
            if path.name!='verify.py':assert digest(path)==h,(str(path),'changed original input')
    (out/('resume-inputs.json' if args.resume else 'inputs.json')).write_text(json.dumps(seal,indent=2))
    receipts=json.loads((out/'receipts.json').read_text()) if args.resume else []
    completed={r['id'] for r in receipts if r.get('passed') or r.get('disposition')}

    def run(id,argv,row):
        if id in completed:return
        cell=out/id;cell.mkdir()
        rec=dict(id=id,argv=list(map(str,argv)),expected={k:row[k] for k in ('status','stdout','stderr')},started=1,completed=0,passed=0)
        receipts.append(rec)
        (out/'receipts.json').write_text(json.dumps(receipts,indent=2))
        def finish(actual, status):
            expected={k:row[k] for k in ('status','stdout','stderr')}
            rec.update(completed=1,actual=actual,passed=int(actual==expected and status==row['status']))
            (out/'receipts.json').write_text(json.dumps(receipts,indent=2))
            assert rec['passed'],(id,actual,expected)
            print('PACKTREE_29_PASS '+id,flush=True)
        if str(argv[0]) == str(args.hirsel) and '--replay' in argv:
            assert argv[-2:] == ['--max-steps','1000000']
            host.add(argv[argv.index('--replay')+1], cell/'result.json', lambda actual: finish(actual, actual['status']))
            return
        command=[sys.executable,str(root/'capped'),'--timeout','60','prlimit','--cpu=60:61',sys.executable,str(root/'verify.py'),'--child',str(cell/'result.json'),'--',*map(str,argv)]
        done=subprocess.run(command,env=env,capture_output=True,timeout=90)
        (cell/'resources').write_bytes(done.stderr)
        assert (cell/'result.json').exists(),(id,done)
        finish(json.loads((cell/'result.json').read_text()), done.returncode)
    tapes=[]
    for backend,binary in binaries.items():
        for row in rows:
            widths=(1,2,4) if row['id'] in ('pack-go','ancestor') else (1,)
            for width in widths:
                id=f"{backend}-{row['id']}-w{width}"
                mounts=out/(id+'-mounts')
                if id in completed:
                    assert not mounts.exists(),id
                    tapes.append((id,out/(id+'.tape'),row,mounts))
                    continue
                mounts.mkdir()
                for name in ('source','archives','target'): (mounts/name).mkdir()
                for p,b in row['files'].items():
                    dest=mounts/p;dest.parent.mkdir(parents=True,exist_ok=True);dest.write_bytes(bytes.fromhex(b))
                for p in row['directories']: (mounts/p).mkdir(parents=True,exist_ok=True)
                for p,target in row['links'].items():
                    dest=mounts/p
                    if dest.exists(): dest.unlink()
                    dest.symlink_to(target)
                grants=dict(io=dict(stdout='stdout.bytes@1',stderr='stderr.bytes@1',tasks='tasks.cooperative@1',
                    dirs=[] if row.get('denied') else [dict(path=n,host=str(mounts/n),access='read' if n=='source' else 'read_write') for n in ('source','archives','target')]),
                    args=row['args'],placement=dict(cores=width,commit='shared',reorder='auto',metrics=str(out/(id+'.metrics'))))
                gp=out/(id+'.grants.json');gp.write_text(json.dumps(grants,indent=2))
                tape=out/(id+'.tape')
                run(id,[binary,'--hirsel-record',tape,'--hirsel-grants',gp,*row['args']],row)
                if row['status']==0 and row['args'][0]=='pack':
                    packed=(mounts/'archives/out.tar').read_bytes()
                    with tarfile.open(fileobj=io.BytesIO(packed),mode='r:') as reader:
                        entries=[]
                        for member in reader:
                            assert member.uid==member.gid==member.mtime==0
                            assert member.uname==member.gname==''
                            b=b'' if member.isdir() else reader.extractfile(member).read()
                            entries.append((member.name,'directory' if member.isdir() else 'file',b))
                        assert report(entries)==row['stdout']
                    if row['id']=='pack-go':
                        prior=out/(backend+'-deterministic.tar')
                        if prior.exists(): assert prior.read_bytes()==packed
                        else: prior.write_bytes(packed)
                if row['status']==0 and row['args'][0]=='unpack':
                    target=mounts/'target'
                    assert not (target/'.packtree-incomplete').exists()
                    assert (target/'.packtree-manifest.json').read_text()==row['stdout']
                    for p,k,b in row['entries']:
                        if k=='file': assert (target/p).read_bytes()==bytes.fromhex(b)
                        else: assert (target/p).is_dir()
                if row['status']!=0 and row['args'][0]=='unpack':
                    assert sorted(str(p.relative_to(mounts/'target')) for p in (mounts/'target').rglob('*')) == (['old'] if row['id']=='target-nonempty' else [])
                mounts.rename(out/(id+'-unavailable'))
                tapes.append((id,tape,row,mounts))
    for id,tape,row,mounts in tapes:
        original=digest(tape)
        for backend,binary in binaries.items():
            run(id+'-replay-'+backend,[binary,'--hirsel-replay',tape],row)
            assert not mounts.exists() and digest(tape)==original
        if row['id'] in full_ids:continue
        run(id+'-replay-reference',[args.hirsel,'run',root/'hirsel.json','--replay',tape,'--max-steps','1000000'],row)
        assert not mounts.exists() and digest(tape)==original
    host.flush()
    tape=tapes[0][1]
    damaged=out/'truncated.tape';damaged.write_bytes(tape.read_bytes()[:-1])
    refusal=subprocess.run([str(binaries['llvm']),'--hirsel-replay',str(damaged)],capture_output=True,env=env,timeout=60)
    assert refusal.returncode!=0 and not refusal.stdout
    (out/'truncated.stderr').write_bytes(refusal.stderr)
    # Refusal controls use the supported CLI; original files stay unavailable.
    changed=out/'changed';changed.mkdir()
    for p in root.glob('*.hir'): (changed/p.name).write_bytes(p.read_bytes())
    manifest=json.loads((root/'hirsel.json').read_text());(changed/'hirsel.json').write_text(json.dumps(manifest))
    entry=changed/'manifest.hir';entry.write_text(entry.read_text().replace('pub const MEMBERS: u64 = 64','pub const MEMBERS: u64 = 63'))
    controls=[('changed-program',[str(args.hirsel),'run',str(changed/'hirsel.json'),'--replay',str(tape)]),
              ('changed-config',[str(binaries['llvm']),'--hirsel-replay',str(tape),'--hirsel-grants',str(out/'llvm-pack-empty-w1.grants.json'),'changed-input'])]
    for name,argv in controls:
        done=subprocess.run(argv,capture_output=True,env=env,timeout=60)
        (out/(name+'.stdout')).write_bytes(done.stdout);(out/(name+'.stderr')).write_bytes(done.stderr)
        assert done.returncode!=0 and not done.stdout,(name,done)
    assert all(digest(Path(p))==h for p,h in seal.items())
    print(json.dumps(dict(**receipt_summary(receipts),
        incomplete=['pending regular-file worker cancellation: no provider readiness script',
                    'physical effective widths: metrics retained, no claimed placement',
                    'checked-H host-tape replay unsupported','scripted native file faults unsupported'])))


if __name__=='__main__':
    if len(sys.argv)>1 and sys.argv[1]=='--child':
        done=subprocess.run(sys.argv[4:],capture_output=True)
        Path(sys.argv[2]).write_text(json.dumps(dict(status=done.returncode,stdout=done.stdout.decode(),stderr=done.stderr.decode())))
        sys.exit(done.returncode)
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--resume',action='store_true')
    parser.add_argument('--root',type=Path,required=True)
    parser.add_argument('--hirsel',type=Path,required=True)
    main(parser.parse_args())
