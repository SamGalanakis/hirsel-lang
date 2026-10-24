"""Production Ordered launches, immutable replay, and independent Unix peers."""
import hashlib, json, os, subprocess, sys
from pathlib import Path
from host import host_case
from model import vectors

from selection import select_cells, check_receipts

def main(cli, manifest, requested):
    root = Path(manifest).parent
    cases = json.loads((root/'cases.json').read_text())
    env = {k:v for k,v in os.environ.items() if not k.startswith('HIRSEL_RT_')}
    env.update(HIRSEL_JOBS='4',HIRSEL_MEMORY_LIMIT='2G',HIRSEL_DAEMON='off',HIRSEL_RESULTS='none@1',HIRSEL_TIMING='off')
    capped=root/'capped';capped.chmod(0o755)
    selection = select_cells(cases, requested)
    receipts=[]
    def save(): (root/'receipts.json').write_text(json.dumps(receipts,indent=2)+'\n')
    def run(cmd, stem, timeout=20, measured=True):
        (root/(stem+'.argv.json')).write_text(json.dumps(cmd)+'\n')
        if measured: cmd=[str(capped),'--timeout','15','prlimit','--cpu=60:61',*cmd]
        try:
            completed=subprocess.run(cmd, env=env,capture_output=True,timeout=timeout)
            code=completed.returncode;out=completed.stdout;err=completed.stderr
        except subprocess.TimeoutExpired as error:
            code=124;out=error.stdout or b'';err=error.stderr or b''
        (root/(stem+'.stdout')).write_bytes(out)
        (root/(stem+'.stderr')).write_bytes(err)
        (root/(stem+'.status')).write_text(str(code))
        return code,out,err

    def digest(path): return hashlib.sha256(path.read_bytes()).hexdigest()
    def tool_seal(label):
        files, directories, links = {}, {}, {}
        def tree(path, ancestors=frozenset()):
            if path.is_symlink(): links[str(path)] = os.readlink(path)
            if path.is_file():
                files[str(path)] = {'sha256': digest(path), 'mode': path.stat().st_mode & 0o777,
                                    'resolved': str(path.resolve())}
            elif path.is_dir():
                identity = (path.stat().st_dev, path.stat().st_ino)
                if identity in ancestors: return
                children = sorted(path.iterdir())
                directories[str(path)] = [child.name for child in children]
                for child in children: tree(child, ancestors | {identity})
        for path in [Path(cli), *[Path(env[key]) for key in
                     ['HIRSEL_PRODUCTS', 'LLVM_SYS_231_PREFIX', 'HIRSEL_RUNTIME_ARCHIVES', 'HIRSEL_LINK_KIT']]]:
            tree(path)
        seal = {'cwd': os.getcwd(), 'files': files, 'directories': directories, 'symlinks': links}
        (root/(label+'.tools.json')).write_text(json.dumps(seal,indent=2)+'\n')
        return seal

    pre_tools = tool_seal('prebuild')
    source_seal={str(p):digest(p) for p in [*root.glob('*.hir'),root/'hirsel.json',root/'cases.json',root/'verify.py',root/'host.py',root/'model.py',root/'selection.py',capped]}
    (root/'build.source.seal.json').write_text(json.dumps(source_seal,indent=2)+'\n')
    vectors()
    binaries={}
    for backend in ['llvm','cranelift']:
        exe=root/backend
        code,out,err=run([cli,'build',manifest,'--backend',backend,'-o',str(exe)],backend+'-build',300,False)
        receipts.append({'id':backend+'-build','status':code,'passed':code==0})
        save()
        if code==0:binaries[backend]=exe
    for path,sha in source_seal.items(): assert digest(Path(path))==sha,('build source changed',path)
    source_seal.update({str(p):digest(p) for p in binaries.values()})
    (root/'runtime.seal.json').write_text(json.dumps(source_seal,indent=2)+'\n')
    for backend,exe in binaries.items():
        for case in cases:
            for width in [1,2,4]:
                name=f'{case["id"]}-{backend}-{width}'
                if name not in selection: continue
                print('PUBSUB_30_STARTED',name,flush=True)
                clients=[{'address':'0.0.0.0','port':0,'tls':'plain','steps':s} for s in case['scripts']]
                grants={'io':{'dirs':[],'stdout':'stdout.bytes@1','tasks':'tasks.cooperative@1','monotonic_clock':'clock.monotonic.ns@1','net':{'provider':'net.script@2','rules':[] if case['id']=='denied' else [{'address':'/fixture/pubsub.sock','service':'0','operation':'unix_listen'}],'peers':[],'listeners':[{'address':'/fixture/pubsub.sock','port':0,'clients':clients}]}},'placement':{'cores':width,'commit':'shared','reorder':'auto'}}
                grant=root/(name+'.json');grant.write_text(json.dumps(grants))
                tape=root/(name+'.tape')
                code,out,err=run([str(exe),'--hirsel-record',str(tape),'--hirsel-grants',str(grant),*case['args']],name)
                receipt={'id':name,'engine':backend,'provider':'script','width_requested':width,'capacity':{'kind':'not_physical','reason':'scripted_world','requested':width,'host_obligation':f'host-{"cancel" if case["reason"]=="deadline" else "domain" if case["exit"]!=0 or case["reason"]=="eof" else "success"}-{backend}-{width}'},'exit':code,'passed':False}
                try:
                    assert code==case['exit'],(code,case['exit'])
                    rows=[json.loads(s) for s in out.splitlines()]
                    if case['exit']==0:
                        final=rows[-1]
                        assert final['joined'] and final['credits_held']==0 and final['reserved']==final['returned'],rows
                        assert final['queued']==final['written']+final['disconnected']==case['queued'],rows
                        assert final['written']==case['queued'] and final['admitted']==case['admitted'],rows
                        assert final['reason']==case['reason'],rows
                    elif case['exit']==2: assert rows==[{'error':'usage_or_limit'}],rows
                    else: assert rows==[{'error':'io'}],rows
                    receipt['passed']=True
                except (AssertionError,IndexError,KeyError,ValueError) as error:receipt['error']=str(error)
                receipts.append(receipt);save()
                grant.unlink() # original provider configuration unavailable for replay
                if not receipt['passed']:continue
                original=tape.read_bytes()
                for target,replayer in binaries.items():
                    stem=name+'-replay-'+target
                    rcode,rout,rerr=run([str(replayer),'--hirsel-replay',str(tape)],stem)
                    receipts.append({'id':stem,'exit':rcode,'passed':(rcode,rout)==(code,out) and not Path('/fixture/pubsub.sock').exists(),'no_effects':not Path('/fixture/pubsub.sock').exists()})
                    save()
                assert tape.read_bytes()==original
                print('PUBSUB_30_PASS' if receipt['passed'] else 'PUBSUB_30_FAIL',name,flush=True)
        for width in [1,2,4]:
            for mode in ['success','domain','cancel']:
                name=f'host-{mode}-{backend}-{width}'
                if name not in selection: continue
                print('PUBSUB_30_STARTED',name,flush=True)
                try:
                    receipt=host_case(exe,root,width,mode,env,capped)
                    receipts.append(receipt);save()
                    tape=Path(receipt['tape']);original=tape.read_bytes()
                    for target,replayer in binaries.items():
                        stem=name+'-replay-'+target
                        code,out,err=run([str(replayer),'--hirsel-replay',str(tape)],stem)
                        receipts.append({'id':stem,'exit':code,'passed':code==0 and out.decode()==receipt['stdout']});save()
                    assert tape.read_bytes()==original
                except (AssertionError,OSError,subprocess.TimeoutExpired,ValueError) as error:
                    receipts.append({'id':name,'passed':False,'disposition':'INCOMPLETE','error':repr(error)});save()
    for path,sha in source_seal.items():assert digest(Path(path))==sha,('source changed during execution',path)
    assert tool_seal('postruntime') == pre_tools, 'compiler/std/runtime/linker/helper bytes or membership changed'
    print(json.dumps({'selected':len(receipts),'passed':sum(r['passed'] for r in receipts),'failed':sum(not r['passed'] for r in receipts)}),flush=True)
    check_receipts(selection, receipts)
    return 0

if __name__ == '__main__':
    sys.exit(main(sys.argv[1], sys.argv[2], tuple(sys.argv[3:])))
