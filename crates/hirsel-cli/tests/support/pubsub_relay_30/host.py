import json, os, selectors, socket, struct, subprocess, tempfile, time
from pathlib import Path

def send(peer, message):
    data=json.dumps(message,separators=(',',':'),ensure_ascii=False).encode()
    assert len(data)<=2048
    peer.sendall(struct.pack('>I',len(data))+data)

def recv(peer):
    def exact(n):
        out=b''
        while len(out)<n:
            more=peer.recv(n-len(out)); assert more, 'unexpected eof'
            out+=more
        return out
    n=struct.unpack('>I',exact(4))[0]; assert 0<n<=2048
    return json.loads(exact(n))

def control(path):
    with socket.socket(socket.AF_UNIX) as peer:
        peer.settimeout(2);peer.connect(str(path));peer.sendall(b'{"metrics":true}\n')
        data=b''
        while b'\n' not in data:
            part=peer.recv(8192);assert part;data+=part
        return json.loads(data)

def host_case(exe, root, width, mode, env, capped):
    name=f'host-{mode}-{exe.name}-{width}'
    directory=root/name;directory.mkdir()
    socket_dir=Path(tempfile.mkdtemp(prefix='ps30-'))
    sock=socket_dir/'relay.sock'; ctl=socket_dir/'ctl'
    grants={'io':{'dirs':[],'stdout':'stdout.bytes@1','tasks':'tasks.cooperative@1','monotonic_clock':'clock.monotonic.ns@1','net':{'provider':'net.stream@1','rules':[{'address':str(sock),'service':'0','operation':'unix_listen'}]}},'placement':{'cores':width,'commit':'shared','reorder':'auto','control':str(ctl)}}
    grant=directory/'grants.json';grant.write_text(json.dumps(grants))
    tape=root/(name+'.tape')
    argv=[str(capped),'--timeout','15','prlimit','--cpu=60:61',str(exe),'--hirsel-record',str(tape),'--hirsel-grants',str(grant),'serve',str(sock),'64','5000' if mode!='cancel' else '500','4']
    (directory/'argv.json').write_text(json.dumps(argv))
    child=subprocess.Popen(argv,env=env,stdout=subprocess.PIPE,stderr=subprocess.PIPE)
    peers=[]
    try:
        sel=selectors.DefaultSelector();sel.register(child.stdout,selectors.EVENT_READ)
        assert sel.select(10), 'no application readiness'
        line=child.stdout.readline();(directory/'first').write_bytes(line);assert json.loads(line)=={'listening':True},line
        samples=[]
        until=time.monotonic()+2
        while time.monotonic()<until:
            snapshot=control(ctl);samples.append(snapshot)
            if snapshot.get('worker_phase')=='running' and snapshot['width']['effective']==width and snapshot['width']['pending']==0:break
        (directory/'control.json').write_text(json.dumps(samples,indent=2))
        settled=samples[-1]
        assert settled.get('worker_phase')=='running' and settled['width']['effective']==width and settled['width']['pending']==0, ('INCOMPLETE width',samples)
        def connect():
            peer=socket.socket(socket.AF_UNIX);peer.settimeout(2);peer.connect(str(sock));peers.append(peer);return peer
        if mode=='success':
            a,b,p=connect(),connect(),connect()
            for peer in [a,b]:send(peer,{'op':'subscribe','topic':'a'});assert recv(peer)=={'op':'subscribed','topic':'a'}
            send(a,{'op':'subscribe','topic':'a'});assert recv(a)['op']=='subscribed'
            delivered=[[],[]]
            for seq,data in [(1,'one'),(2,'two')]:
                send(p,{'op':'publish','topic':'a','data':data});assert recv(p)=={'op':'queued','seq':seq,'subscribers':2}
                for i,peer in enumerate([a,b]):
                    assert recv(peer)=={'op':'delivery','seq':seq,'topic':'a','data':data};delivered[i].append(seq)
            send(b,{'op':'unsubscribe','topic':'a'});assert recv(b)['op']=='unsubscribed'
            send(p,{'op':'publish','topic':'a','data':'three'});assert recv(p)=={'op':'queued','seq':3,'subscribers':1};assert recv(a)['seq']==3
            (directory/'deliveries.json').write_text(json.dumps([[1,2,3],[1,2]]))
            send(p,{'op':'stop'});assert recv(p)=={'op':'stopping'}
        elif mode=='domain':
            bad=connect();bad.sendall(b'\0\0\0\1{');assert bad.recv(1)==b''
            p=connect();send(p,{'op':'stop'});assert recv(p)=={'op':'stopping'}
        else:
            p=connect();p.sendall(b'\0\0')  # genuinely pending read, no injected Canceled
        suffix,stderr=child.communicate(timeout=12)
        output=line+suffix
        (directory/'stdout').write_bytes(output);(directory/'stderr').write_bytes(stderr);(directory/'status').write_text(str(child.returncode))
        assert child.returncode==0,(child.returncode,stderr,output)
        rows=[json.loads(s) for s in output.splitlines()]; report=rows[-1]
        assert report['joined'] and report['credits_held']==0 and report['reserved']==report['returned'],rows
        assert report['queued']==report['written']+report['disconnected'],rows
        assert report['reason']==('deadline' if mode=='cancel' else 'stop'),rows
        grant.unlink();assert not sock.exists(), 'live listener after joined shutdown'
        return {'id':name,'stdout':output.decode(),'exit':0,'tape':str(tape),'width':settled,'passed':True}
    finally:
        for peer in peers:peer.close()
        if child.poll() is None: child.kill()
        tail,diagnostic=child.communicate()
        if not (directory/'stderr').exists():
            (directory/'stdout-tail').write_bytes(tail);(directory/'stderr').write_bytes(diagnostic);(directory/'status').write_text(str(child.returncode))
        if not sock.exists() and not ctl.exists(): socket_dir.rmdir()
