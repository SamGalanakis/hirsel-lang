"""Separate capped diagnostics and observe admission via inotify/control handshake."""
import ctypes
import json
import os
from pathlib import Path
import select
import socket
import subprocess
import sys
import time

root=Path(sys.argv[1]);argv=sys.argv[2:];control=None;notify=None
if '--hirsel-grants' in argv:
    config=json.loads(Path(argv[argv.index('--hirsel-grants')+1]).read_text())
    control=config.get('placement',{}).get('control')
if control:
    libc=ctypes.CDLL(None,use_errno=True)
    notify=libc.inotify_init1(os.O_CLOEXEC)
    if notify<0:raise OSError(ctypes.get_errno(),'inotify_init1')
    if libc.inotify_add_watch(notify,os.fsencode(Path(control).parent),0x100)<0:
        raise OSError(ctypes.get_errno(),'inotify_add_watch')
p=subprocess.Popen(argv,stdin=subprocess.DEVNULL,stdout=subprocess.PIPE,stderr=subprocess.PIPE)
if notify is not None:
    pidfd=os.pidfd_open(p.pid)
    try:
        while True:
            ready,_,_=select.select([notify,pidfd],[],[],10)
            if not ready:
                (root/'admission-incomplete.txt').write_text('No admission handshake before bounded wait expired')
                break
            if notify in ready:
                os.read(notify,65536)
                if Path(control).exists():
                    try:
                        samples=[]
                        deadline=time.monotonic()+5
                        requested=config['placement']['cores']
                        qualified=False
                        while time.monotonic()<deadline:
                            with socket.socket(socket.AF_UNIX) as peer:
                                peer.settimeout(max(0.001,deadline-time.monotonic()))
                                peer.connect(control);peer.sendall(b'{"metrics":true}\n')
                                reply=b''
                                while not reply.endswith(b'\n'):
                                    part=peer.recv(65536)
                                    if not part:break
                                    reply+=part
                            snapshot=json.loads(reply)
                            samples.append(snapshot)
                            (root/'admission-samples.json').write_text(json.dumps(samples))
                            (root/'admission.json').write_bytes(reply)
                            width=snapshot['width']
                            qualified=(snapshot.get('worker_phase')=='running' and
                                       width['requested']==requested and
                                       width['effective']==requested and width['pending']==0)
                            if qualified or snapshot.get('worker_phase') in ('draining','finished'):
                                break
                            if select.select([pidfd],[],[],0)[0]:break
                        (root/'width-qualified.json').write_text(json.dumps(qualified))
                        if not qualified:
                            (root/'admission-incomplete.txt').write_text('No running settled-width sample before exit or bounded deadline')
                    except (OSError, ValueError) as error:
                        (root/'admission-incomplete.txt').write_text(str(error))
                    break
            if pidfd in ready:
                (root/'admission-incomplete.txt').write_text('Application exited before admission control reply')
                break
    finally:
        os.close(pidfd);os.close(notify)
stdout,stderr=p.communicate()
(root/'stdout').write_bytes(stdout);(root/'stderr').write_bytes(stderr)
(root/'status.json').write_text(json.dumps(p.returncode))
# Wrapper success is setup only; Rust checks the actual status.json value.
