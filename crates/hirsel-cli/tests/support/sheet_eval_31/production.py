"""Ordinary Ordered full app launches. Capacity receipts are held by stdin handshake."""
import argparse,json,os,selectors,signal,socket,struct,subprocess,sys,time
from pathlib import Path
p=argparse.ArgumentParser();p.add_argument('--app',required=True);p.add_argument('--backend',required=True);p.add_argument('--root',required=True);p.add_argument('--capped',required=True);p.add_argument('--workflow',choices=['success','domain','cancel']);a=p.parse_args()
root=Path(a.root);root.mkdir(parents=True,exist_ok=True)
env=os.environ.copy();env.update(HIRSEL_JOBS='4',HIRSEL_MEMORY_LIMIT='2G',HIRSEL_DAEMON='off',HIRSEL_RESULTS='none@1')
for key in ['HIRSEL_RT_WORKERS','HIRSEL_RT_MIGRATE','HIRSEL_FREE_TEST','HIRSEL_FREE_REPLAY']:env.pop(key,None)

def await_line(stream,seconds=30):
    selector=selectors.DefaultSelector();selector.register(stream,selectors.EVENT_READ)
    data=b'';deadline=time.monotonic()+seconds
    while not data.endswith(b'\n'):
        if not selector.select(max(0,deadline-time.monotonic())):raise RuntimeError('handshake timeout')
        b=os.read(stream.fileno(),1)
        if not b:raise RuntimeError('EOF before handshake '+repr(data))
        data+=b
    selector.close();return data

for workflow in ([a.workflow] if a.workflow else ['success','domain','cancel']):
 for width in [1,2,4]:
    label=f'{a.backend}-{workflow}-{width}';d=root/label;d.mkdir(exist_ok=False);work=d/'work';work.mkdir()
    ctl=d/'ctl';tape=d/'recording';grants=d/'grants.json'
    io={'stdout':'stdout.bytes@1','stdin':'stdin.bytes@1','tasks':'tasks.cooperative@1','dirs':[dict(path='work',host=str(work.resolve()),access='read_write')]}
    args=['--jobs','4']+(['--interruptible'] if workflow=='cancel' else [])+['-']
    if workflow=='cancel':io['signals']={'provider':'signals.host@1','kinds':['interrupt']}
    grants.write_text(json.dumps(dict(args=args,io=io,placement=dict(cores=width,control=str(ctl),commit='shared',reorder='auto'))))
    argv=[a.capped,'--timeout','60','prlimit','--cpu=60:61',a.app,'--hirsel-record',str(tape),'--hirsel-grants',str(grants),*args]
    (d/'argv.json').write_text(json.dumps(argv))
    with (d/'stderr').open('wb') as errors:
      child=subprocess.Popen(argv,env=env,stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=errors,bufsize=0)
      try:
        child.stdin.write(b'recalc\n');child.stdin.flush();head=await_line(child.stdout)
        assert head==b'RECALC 0\n',head
        connection=socket.socket(socket.AF_UNIX,socket.SOCK_STREAM);connection.settimeout(10);connection.connect(str(ctl));connection.sendall(b'{"metrics":true}\n')
        reply=b''
        while not reply.endswith(b'\n'):reply+=connection.recv(8192)
        server_pid=struct.unpack('3i',connection.getsockopt(socket.SOL_SOCKET,socket.SO_PEERCRED,12))[0];connection.close();(d/'control.json').write_bytes(reply);metrics=json.loads(reply)
        assert metrics['worker_phase']=='running' and metrics['width']['effective']==width and metrics['width']['pending']==0,metrics
        if workflow=='cancel':
          # SO_PEERCRED identifies the exact serving app process.
          os.kill(server_pid,signal.SIGINT);expected=head+b'error: canceled\n';code=3
        else:
          script=b'set A1 = 2\nset B1 = A1+3\nshow\nsave book.json\nload book.json\nshow B1\n' if workflow=='success' else b'set A1 = 1/0\nshow A1\n'
          child.stdin.write(script);child.stdin.flush();expected=head+(b'OK\nOK\nA1 = 2\nB1 = 5\nOK\nOK\nB1 = 5\n' if workflow=='success' else b'OK\nA1 = #DIV_ZERO\n');code=0 if workflow=='success' else 1
        child.stdin.close();tail=child.stdout.read();rc=child.wait(timeout=65)
        output=head+tail;(d/'stdout').write_bytes(output);(d/'exit').write_text(str(rc))
        assert rc==code,(label,rc,(d/'stderr').read_text())
        assert output==expected,(label,output,expected)
        assert not ctl.exists(),'control cleanup'
        (d/'PASS').write_text('capacity qualified; no claim of simultaneous CPU utilization\n')
        print('PRODUCTION_PASS',label,flush=True)
      except BaseException:
        if child.poll() is None:child.terminate();child.wait(timeout=15)
        raise
