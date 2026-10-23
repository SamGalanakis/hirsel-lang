"""Ordinary Ordered full-app launches, fixed jobs2, requested width1/2/4.
Read the application-ready line before requesting settled worker diagnostics.
No worker test knobs, no Free, no sleeps. Real input files are removed for replay.
"""
import json, os, pathlib, socket, subprocess, sys
cli, manifest = sys.argv[1:]
root = pathlib.Path(manifest).parent
(root/'input').mkdir()
receipt = os.environ.get('TOY_ANALYZER_31_READY_RECEIPT') == '1'
if receipt: (root/'output').mkdir()
(root/'input/modules.json').write_text('{"modules":[{"name":"a","path":"a.toy"}]}')
env = dict(os.environ, HIRSEL_JOBS='4', HIRSEL_MEMORY_LIMIT='2G')
for key in ('HIRSEL_RT_WORKERS', 'HIRSEL_RT_MIGRATE', 'HIRSEL_FREE_TEST'):
    env.pop(key, None)
for backend in ('llvm','cranelift'):
    app = root/backend
    built = subprocess.run([cli,'build',str(manifest),'--backend',backend,'-o',str(app)],env=env,capture_output=True)
    assert built.returncode == 0, built.stderr.decode()
    for workflow, source, expected in [('success','let x=1;',0),('domain','let x=y;',1),('cancel',' '*8192,2),('cancel-default',' '*8192,2)]:
        for width in (1,2,4):
            ident = f'{backend}-{workflow}-w{width}'
            selected = os.environ.get('TOY_ANALYZER_31_PRODUCTION_CELLS')
            if selected and ident not in selected.split(','): continue
            (root/'input/a.toy').write_text(source)
            modules=[dict(name='a',path='a.toy')]
            if workflow.startswith('cancel'):
                (root/'input/b.toy').write_text('{'*24+'let x=1;'+'}'*24)
                modules.append(dict(name='b',path='b.toy'))
            manifest_input=dict(modules=modules)
            ready_receipt=receipt and workflow!='cancel-default'
            if ready_receipt: manifest_input['ready']='admitted'
            (root/'input/modules.json').write_text(json.dumps(manifest_input))
            ctl=root/(ident+'.ctl'); tape=root/(ident+'.tape')
            grants=root/(ident+'.grants.json')
            dirs=[dict(path='input',host=str(root/'input'),access='read')]
            if receipt: dirs.append(dict(path='output',host=str(root/'output'),access='read_write'))
            grants.write_text(json.dumps(dict(args=['--ready','check','modules.json','2'],io=dict(stdout='stdout.bytes@1',stdin='stdin.bytes@1',tasks='tasks.cooperative@1',dirs=dirs),placement=dict(cores=width,commit='shared',reorder='off',control=str(ctl)))))
            command=[sys.executable,str(root/'capped'),'--timeout','60','prlimit','--cpu=60:61',str(app),'--hirsel-record',str(tape),'--hirsel-grants',str(grants),'--ready','check','modules.json','2']
            child=subprocess.Popen(command,stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=subprocess.PIPE,env=env)
            assert child.stdout.readline()==b'ready\n',ident
            with socket.socket(socket.AF_UNIX) as conn:
                conn.settimeout(5);conn.connect(str(ctl));conn.sendall(b'{"metrics":true}\n');reply=json.loads(conn.makefile('rb').readline())
            (root/(ident+'.control.json')).write_text(json.dumps(reply))
            capacity_ok=reply['worker_phase']=='running' and reply['width']['effective']==width and reply['width']['pending']==0
            if not capacity_ok:
                child.stdin.close();child.stdin=None;child.communicate(timeout=65)
            assert capacity_ok,reply
            child.stdin.write(b'x');child.stdin.flush();out,err=child.communicate(timeout=65)
            (root/(ident+'.stdout')).write_bytes(b'ready\n'+out);(root/(ident+'.stderr')).write_bytes(err)
            assert child.returncode==expected,(ident,child.returncode,err)
            if workflow.startswith('cancel'): assert json.loads(out)==dict(error='ast_depth'),(ident,out)
            else: assert json.loads(out)['total']==expected,(ident,out)
            if ready_receipt: assert (root/'output/admitted').read_bytes()==b'ready'
            original=tape.read_bytes()
            if ready_receipt: (root/'output/admitted').unlink()
            (root/'input/a.toy').unlink()
            (root/'input/b.toy').unlink(missing_ok=True)
            for target in ('llvm','cranelift'):
                if not (root/target).exists(): continue
                replay=subprocess.run([sys.executable,str(root/'capped'),'--timeout','60','prlimit','--cpu=60:61',str(root/target),'--hirsel-replay',str(tape)],env=env,capture_output=True)
                assert replay.returncode==expected and replay.stdout==b'ready\n'+out,(ident,target,replay)
            assert tape.read_bytes()==original
            if receipt: assert not (root/'output/admitted').exists()
            print('TOY_PRODUCTION_PASS',ident,json.dumps(reply),flush=True)
