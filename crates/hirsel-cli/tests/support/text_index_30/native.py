"""Bounded production launches of the complete textindex application.
Every recording is immutable and kept outside mutable grants. Width evidence is
sampled from the ordinary control endpoint; startup samples never count ready.
"""
import argparse,hashlib,json,os,resource,selectors,socket,subprocess,sys,time,tempfile,shutil
from pathlib import Path


def digest(p):
    h=hashlib.sha256()
    with p.open('rb') as f:
        for block in iter(lambda:f.read(1048576),b''):h.update(block)
    return h.hexdigest()


def main(a):
    root=a.root.resolve();cli=a.hirsel.resolve();backend=a.backend
    env={k:v for k,v in os.environ.items() if not k.startswith('HIRSEL_RT_')}
    env.update(HIRSEL_DAEMON='off',HIRSEL_JOBS='4',HIRSEL_MEMORY_LIMIT='2G')
    evidence=root/('native-'+backend);evidence.mkdir()
    control_scratch=tempfile.TemporaryDirectory(prefix='ti30-')
    receipts=[]
    def save(): (evidence/'receipts.json').write_text(json.dumps(receipts,indent=2)+'\n')
    def run(label,argv,runtime=True,placement=None):
        label=label.replace('/','-');out=evidence/(label+'.stdout');err=evidence/(label+'.stderr');samples=[]
        inputs=[cli,root/'hirsel.json',root/'cases.json',root/'native.py',root/'capped',*root.glob('*.hir')]
        binary=evidence/backend
        if binary.exists():inputs.append(binary)
        inputs.extend(p for p in root.rglob('*.grants.json'))
        inputs.extend(p for directory in evidence.glob('provider-*') for p in directory.rglob('*') if p.is_file())
        receipt=dict(id=label,backend=backend,argv=list(map(str,argv)),state='started',inputs={str(p):digest(p) for p in inputs},width_samples=samples)
        receipts.append(receipt);save()
        def limits(): resource.setrlimit(resource.RLIMIT_CPU,(60,61))
        command=[sys.executable,str(root/'capped'),'--timeout','60' if runtime else '300',*map(str,argv)]
        # Record target streams independently of capped's resource receipt.
        child=root/'capture.py'
        if placement:
            # Control samples belong to host diagnostics, outside canonical tape.
            proc=subprocess.Popen(command,cwd=root,env=env,stdout=subprocess.PIPE,stderr=subprocess.PIPE,preexec_fn=limits if runtime else None)
            deadline=time.monotonic()+65
            while proc.poll() is None and time.monotonic()<deadline:
                try:
                    with socket.socket(socket.AF_UNIX) as s:
                        s.settimeout(.05);s.connect(str(placement));s.sendall(b'{"metrics":true}\n');reply=b''
                        while not reply.endswith(b'\n'):
                            part=s.recv(65536)
                            if not part:break
                            reply+=part
                        sample=json.loads(reply);samples.append(sample)
                        if sample.get('worker_phase')=='running' and sample.get('width',{}).get('pending')==0:break
                except (OSError,ValueError):pass
            stdout,stderr=proc.communicate(timeout=max(1,deadline-time.monotonic()))
            result=subprocess.CompletedProcess(command,proc.returncode,stdout,stderr)
        else:
            result=subprocess.run(command,cwd=root,env=env if runtime else env|{'HIRSEL_MEMORY_LIMIT':'4G'},stdin=subprocess.DEVNULL,capture_output=True,timeout=330 if not runtime else 75,preexec_fn=limits if runtime else None)
        out.write_bytes(result.stdout);err.write_bytes(result.stderr)
        receipt.update(state='completed',status=result.returncode)
        save();return result,receipt
    tool_files=[cli,root/'capped',root/'capture.py',Path(sys.executable).resolve()]
    for key in ['HIRSEL_PRODUCTS','HIRSEL_RUNTIME_ARCHIVES','HIRSEL_LINK_KIT','LLVM_SYS_231_PREFIX']:
        if not env.get(key):continue
        path=Path(env[key]).resolve()
        tool_files.extend(p for p in path.rglob('*') if p.is_file()) if path.is_dir() else tool_files.append(path)
    (evidence/'tools-before.json').write_text(json.dumps({str(p):digest(p) for p in tool_files},indent=2))
    executable=evidence/backend
    reuse=os.environ.get('TEXT_INDEX_30_BINARY_'+backend.upper())
    if reuse:
        shutil.copy2(reuse,executable)
        receipts.append(dict(id='reuse-build',state='completed',passed=True,source=reuse,sha256=digest(executable)));save()
    else:
        built,row=run('build',[cli,'build',root/'hirsel.json','--backend',backend,'-o',executable],False)
        assert built.returncode==0,built.stderr.decode(errors='replace')
        row['passed']=True;save()
    selected=[c for c in json.loads((root/'cases.json').read_text()) if c['boundary']==a.boundary]
    for case in selected:
        if case.get('pending'):
            receipts.append(dict(id=case['id'],state='INCOMPLETE',reason='scripted Pending is not a host-file provider; see native-plan.md witness'));save();continue
        widths=[1,2,4] if case['id'] in ['tantivy-j4','invalid-utf8'] else [1]
        for width in widths:
            cell=evidence/(case['id']+f'-w{width}');cell.mkdir()
            provider=evidence/('provider-'+case['id']);provider.mkdir(exist_ok=True)
            data=provider/'data';index=provider/'index';data.mkdir();index.mkdir()
            for name,content in case['files'].items():
                p=(data/name[6:]) if name.startswith('input/') else (index/name[6:]);p.parent.mkdir(parents=True,exist_ok=True);p.write_bytes(content.encode() if isinstance(content,str) else bytes(content))
            for stage_number,stage in enumerate(case['stages']):
                label=case['id']+f'-w{width}-s{stage_number}'
                control=Path(control_scratch.name)/f'ctl-{width}-{stage_number}'
                grants=dict(args=stage['args'],io=dict(stdout='stdout.bytes@1',stderr='stderr.bytes@1',tasks='tasks.cooperative@1',dirs=[dict(path='input',host=str(data),access='read'),dict(path='index',host=str(index),access='read' if case.get('access')=='read' else 'read_write')]),placement=dict(cores=width,commit='shared',reorder='auto',control=str(control)))
                if case.get('deny_projection'):grants['io'].pop(case['deny_projection'],None)
                if case.get('access')=='denied':grants['io']['dirs']=grants['io']['dirs'][1:]
                grant=cell/f'{stage_number}.grants.json';grant.write_text(json.dumps(grants)+'\n')
                before={str(p.relative_to(index)):digest(p) for p in index.rglob('*') if p.is_file()}
                tape=evidence/(label+'.tape');target_out=evidence/(label+'.target.stdout');target_err=evidence/(label+'.target.stderr')
                record=case['id'] in ['tantivy-j4','invalid-utf8','corrupt-reference']
                argv=[sys.executable,root/'capture.py',target_out,target_err,executable,*(['--hirsel-record',tape] if record else []),'--hirsel-grants',grant,*stage['args']]
                result,row=run(label,argv,placement=control)
                stdout=target_out.read_bytes() if target_out.exists() else b'';stderr=target_err.read_bytes() if target_err.exists() else result.stderr
                row['expected']=stage
                if record:row['tape']=str(tape)
                if stage.get('index') is not None:okay=json.loads(stdout)==stage['index']
                elif stage.get('rows') is not None:okay=[json.loads(line) for line in stdout.splitlines()]==stage['rows']
                else:okay=stdout==stage['stdout'].encode()
                row['passed']=okay and result.returncode==stage['exit'] and stderr==stage.get('stderr','').encode()
                row['width_qualified']=any(s.get('worker_phase')=='running' and s.get('width',{}).get('effective')==width and s.get('width',{}).get('pending')==0 for s in row['width_samples'])
                save()
                assert row['passed'],(label,result.returncode,stdout[:1000],stderr[:1000],stage)
                if stage['exit']==2:assert before=={str(p.relative_to(index)):digest(p) for p in index.rglob('*') if p.is_file()},label
            # Remove all original providers before replay. Stage tapes include reads;
            # no host input files, grants or writeable output trees remain available.
            data.rename(cell/'unavailable-input');index.rename(cell/'unavailable-index')
    print(json.dumps(dict(completed=sum(r.get('state')=='completed' for r in receipts),passed=sum(r.get('passed',False) for r in receipts),recordings=[r for r in receipts if r.get('tape')]),ensure_ascii=False))

if __name__=='__main__':
    p=argparse.ArgumentParser();p.add_argument('--root',type=Path,required=True);p.add_argument('--hirsel',type=Path,required=True);p.add_argument('--backend',choices=['llvm','cranelift'],required=True);p.add_argument('--boundary',action='store_true');main(p.parse_args())
