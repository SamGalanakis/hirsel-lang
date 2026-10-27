"""Ordinary Ordered full-app launches, fixed jobs2, requested width1/2/4.
Read the application-ready line before requesting settled worker diagnostics.
Send stdin only after its recorded read blocks and home waits in the kernel.
No worker test knobs, no Free, no sleeps. Real input files are removed for replay.
"""
import ctypes, json, os, pathlib, select, socket, struct, subprocess, sys, time
# Adapted from depsolve_29/child.py and chunk_backup_31/production.py.
# tape_binary.rs flushes each completion; observe it without changing the tape.

def stdin_is_blocked(tape):
    data = tape.read_bytes()
    magic = b'hirsel-io-tape 11\0'
    if len(data) < len(magic): return False
    assert data.startswith(magic), 'unexpected tape version'
    at = len(magic)
    expected = 0
    while at + 17 <= len(data):
        length, checksum, seq, kind = struct.unpack_from('<IIQB', data, at)
        assert seq == expected, 'unexpected tape sequence'
        end = at + 17 + length
        if end > len(data): return False
        payload = data[at + 17:end]
        # ReadStdin(stdin: u64, max: u64) followed by Completion::Blocked.
        if kind == 3 and payload[:4] == struct.pack('<I', 2) and payload[20:24] == struct.pack('<I', 37):
            crc = 0xffffffff
            for byte in data[at:at + 4] + data[at + 8:at + 17] + payload:
                crc ^= byte
                for _ in range(8):
                    crc = (crc >> 1) ^ (0x82f63b78 if crc & 1 else 0)
            assert (crc ^ 0xffffffff) == checksum, 'stdin frame CRC'
            return True
        at = end
        expected += 1
    return False

def wait_for_stdin_parked(child, tape, pid):
    libc = ctypes.CDLL(None, use_errno=True)
    notify = libc.inotify_init1(os.O_CLOEXEC | os.O_NONBLOCK)
    if notify < 0: raise OSError(ctypes.get_errno(), 'inotify_init1')
    try:
        # Watch before reading the prefix so no completion wakeup can be lost.
        if libc.inotify_add_watch(notify, os.fsencode(tape), 0x2 | 0x8) < 0:
            raise OSError(ctypes.get_errno(), 'inotify_add_watch')
        deadline = time.monotonic() + 5
        while not stdin_is_blocked(tape):
            assert child.poll() is None, 'application exited before stdin blocked'
            remaining = deadline - time.monotonic()
            assert remaining > 0 and select.select([notify], [], [], remaining)[0], 'stdin did not block'
            os.read(notify, 65536)
    finally:
        os.close(notify)
    # A flushed Blocked event can precede the first empty readiness point.
    # Readiness::block sleeps in epoll_wait; before stdin is released, home
    # has no other application input to wait for. Observe that actual wait.
    while True:
        wait = pathlib.Path(f'/proc/{pid}/wchan').read_text().strip()
        if wait in ('ep_poll', 'do_epoll_wait'): return
        assert child.poll() is None and time.monotonic() < deadline, (pid, wait)
        os.sched_yield()

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
            for fixture_input in (root/'input').iterdir():
                os.utime(fixture_input, ns=(0,0))
            ctl=root/(ident+'.ctl'); tape=root/(ident+'.tape')
            grants=root/(ident+'.grants.json')
            dirs=[dict(path='input',host=str(root/'input'),access='read')]
            if receipt: dirs.append(dict(path='output',host=str(root/'output'),access='read_write'))
            grants.write_text(json.dumps(dict(args=['--ready','check','modules.json','2'],io=dict(stdout='stdout.bytes@1',stdin='stdin.bytes@1',tasks='tasks.cooperative@1',dirs=dirs),placement=dict(cores=width,commit='shared',reorder='off',control=str(ctl)))))
            command=[sys.executable,str(root/'capped'),'--timeout','60','prlimit','--cpu=60:61',str(app),'--hirsel-record',str(tape),'--hirsel-grants',str(grants),'--ready','check','modules.json','2']
            child=subprocess.Popen(command,stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=subprocess.PIPE,env=env)
            assert child.stdout.readline()==b'ready\n',ident
            with socket.socket(socket.AF_UNIX) as conn:
                conn.settimeout(5);conn.connect(str(ctl))
                pid,uid,_=struct.unpack('3i',conn.getsockopt(socket.SOL_SOCKET,socket.SO_PEERCRED,12))
                assert uid==os.getuid()
                conn.sendall(b'{"metrics":true}\n');reply=json.loads(conn.makefile('rb').readline())
            (root/(ident+'.control.json')).write_text(json.dumps(reply))
            capacity_ok=reply['worker_phase']=='running' and reply['width']['effective']==width and reply['width']['pending']==0
            if not capacity_ok:
                child.stdin.close();child.stdin=None;child.communicate(timeout=65)
            assert capacity_ok,reply
            wait_for_stdin_parked(child, tape, pid)
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
