#!/usr/bin/env python3
"""Independent expected statuses; finite direct children, bounded pipes and reaping.

Go os/exec TestPipes stdin-close/output sequence is adapted to discard-only wait.
Every invoked argv is explicit. Kernel creation/wait events account for every child.
"""
import argparse
import copy
import ctypes
import hashlib
import json
import os
from pathlib import Path
import select
import subprocess
import sys
import time

from child_accounting import account


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def verify(args):
    root = args.root
    out = root / 'evidence'
    out.mkdir()
    env = {k: v for k, v in os.environ.items() if not k.startswith('HIRSEL_RT_')}
    env.update(HIRSEL_DAEMON='off', HIRSEL_RESULTS='none@1', HIRSEL_JOBS='1', HIRSEL_MEMORY_LIMIT='2G')
    (out / 'environment.json').write_text(json.dumps(env, indent=2))
    cases = json.loads((root / 'cases.json').read_text())
    if args.case:
        cases = [case for case in cases if case['id'] in args.case]
    if args.caps:
        cases = json.loads((root / 'caps.json').read_text())
    selection = root / 'hirsel.json'
    binary = out / args.engine
    receipts = []
    if args.engine != 'interpreter':
        argv = [str(args.hirsel), 'build', str(selection), '--backend', args.engine, '-o', str(binary)]
        built = subprocess.run(argv, capture_output=True, env=env, timeout=300)
        (out / 'build.stdout').write_bytes(built.stdout)
        (out / 'build.stderr').write_bytes(built.stderr)
        (out / 'build.json').write_text(json.dumps(dict(argv=argv, exit=built.returncode)))
        assert built.returncode == 0, (args.engine, built)
    source_seal = {str(p): digest(p) for p in [selection, root/'main.hir', root/'model.hir', root/'verify.py', root/'child_accounting.py', root/'child.py', root/'cases.json', root/'capped', args.hirsel]}
    if binary.exists():
        source_seal[str(binary)] = digest(binary)
    (out/'source-seal-before.json').write_text(json.dumps(source_seal, indent=2))
    # inotify notifications are readiness handshakes, never fixed startup sleeps.
    libc = ctypes.CDLL(None, use_errno=True)
    for original in cases:
        case = copy.deepcopy(original)
        identity = case['id']
        state = out / identity
        state.mkdir()
        fd = libc.inotify_init1(os.O_CLOEXEC | os.O_NONBLOCK)
        assert fd >= 0
        assert libc.inotify_add_watch(fd, os.fsencode(state), 0x100 | 0x8) >= 0
        def wait_file(path, app, seconds):
            # Go TestPipes reads only after Start and a read ends at child EOF:
            # each wait ends at its marker or at the app's exit (pidfd).
            until = time.monotonic() + seconds
            while not path.exists():
                remaining = until - time.monotonic()
                assert remaining > 0, ('handshake timeout', path)
                ready = select.select([fd, app], [], [], remaining)[0]
                assert ready, ('handshake timeout', path)
                if fd in ready:
                    os.read(fd, 65536)
                assert app not in ready or path.exists(), ('app exited before handshake', path)
        manifest = case['manifest']
        for i, command in enumerate(manifest['commands']):
            mode = case['modes'][i]
            command['argv'] = ['/usr/bin/python3', str(root/'child.py'), mode, str(state), str(i)]
            if case['rows']:
                case['rows'][i]['program'] = '/usr/bin/python3'
            if mode == 'hold':
                os.mkfifo(state / (str(i)+'.release'))
        if identity == 'absent':
            missing = str(state/'absent-child')
            manifest['commands'][0]['argv'] = [missing]
            case['rows'][0]['program'] = missing
        data = state/'manifest.json'
        data.write_text(json.dumps(manifest))
        grants = dict(io=dict(stdout='stdout.bytes@1', stderr='stderr.bytes@1',
            tasks='tasks.cooperative@1', monotonic_clock='clock.monotonic.ns@1',
            dirs=[dict(path='fixtures', host=str(state), access='read')],
            process=dict(provider='process.spawn@1', rules=[] if case['refused'] else [dict(program=missing if identity=='absent' else '/usr/bin/python3')])),
            args=['manifest.json'], placement=dict(cores=1, commit='shared', reorder='auto'))
        grant_file = state/'grants.json'
        grant_file.write_text(json.dumps(grants))
        tape = out/(identity+'.tape')
        expected_file = state/'expected.json'
        expected_file.write_text(json.dumps(dict(exit=case['exit'],rows=case['rows'],stderr=case['stderr']),indent=2))
        if args.engine == 'interpreter':
            argv = [str(args.hirsel),'run',str(selection),'--grants',str(grant_file),'--record',str(tape),'--max-steps','1000000']
        else:
            argv = [str(binary),'--hirsel-record',str(tape),'--hirsel-grants',str(grant_file),'manifest.json']
        stdout, stderr = state/'stdout', state/'stderr'
        receipt = dict(id=identity, engine=args.engine, argv=argv, state='started',
            input_sha256=digest(data), expected_sha256=digest(expected_file), grants_sha256=digest(grant_file),
            requested_width=1, production_test_hooks={k:v for k,v in env.items() if k.startswith('HIRSEL_RT_')})
        receipts.append(receipt)
        (out/'receipts.json').write_text(json.dumps(receipts,indent=2))
        accounting = state/'children.json'
        wrapper = [sys.executable,str(root/'capped'),'--timeout','60',
                   sys.executable,str(root/'child_accounting.py'),str(accounting),*argv]
        with stdout.open('wb') as stdout_file, stderr.open('wb') as stderr_file:
            child = subprocess.Popen(wrapper, stdin=subprocess.DEVNULL, stdout=stdout_file, stderr=stderr_file, env=env)
            if identity == 'mixed':
                # First job is held until the later three have completed. An
                # app that exits early fails at once, not at the 10s bound.
                app = os.pidfd_open(child.pid)
                wait_file(state/'0.started', app, 10)
                for i in [1,3]:
                    wait_file(state/(str(i)+'.done'), app, 10)
                wait_file(state/'2.started', app, 10)
                os.close(app)
                with (state/'0.release').open('wb') as release:
                    release.write(b'x')
            code = child.wait(timeout=75)
        os.close(fd)
        # capped emits resource accounting on stderr; child application stderr is
        # captured independently below by the tape/output contract where applicable.
        rows = [json.loads(line) for line in stdout.read_bytes().splitlines()]
        text = stderr.read_text()
        app_stderr = ''.join(line+'\n' for line in text.splitlines() if not line.startswith('capped:'))
        actual = dict(exit=code, rows=rows, stderr=app_stderr)
        expected = dict(exit=case['exit'],rows=case['rows'],stderr=case['stderr'])
        # Readiness markers are a subset of kernel-created PIDs. They are
        # never the creation boundary or evidence that a child was reaped.
        pids = {p.stem: int(p.read_text()) for p in state.glob('*.started')}
        children = account(json.loads(accounting.read_text()), pids)
        if identity in ['child-deadline', 'global-deadline']:
            for row in children:
                if row['state'] == 'ready then signaled':
                    row['state'] = 'ready then cancelled'
        if identity not in ['invalid','absent','grant-refusal','global-deadline']:
            assert len(children) == len(manifest['commands']), (identity,children)
        elif identity == 'global-deadline':
            assert len(children) <= 2, ('admission cap and canceled queue',children)
        elif identity in ['invalid', 'grant-refusal']:
            assert not children, (identity,children)
        else:
            # ENOENT may be detected before fork or returned by a created
            # child. Any such child still needs exact wait/reap proof.
            assert len(children) <= 1 and all(not row.get('exec') for row in children), children
        creation_state = ('created' if children else
                          'denied' if identity == 'grant-refusal' else
                          'absent' if identity == 'absent' else 'never spawned')
        if identity == 'both-pipes':
            done = json.loads((state/'0.done').read_text())
            assert done['stdout'] == done['stderr'] == 98304
        receipt.update(state='completed', passed=actual==expected, actual=actual, fixture_pids=pids, children=children, creation_state=creation_state, reaped=True,
                       tape_sha256=digest(tape) if tape.exists() else None)
        (out/'receipts.json').write_text(json.dumps(receipts,indent=2))
        assert actual == expected, (identity,args.engine,actual,expected)
        # Host files and the child program are unavailable during replay.
        data.rename(state/'manifest.unavailable')
        (root/'child.py').rename(root/'child.unavailable')
        immutable = tape.read_bytes()
        replay_argv = ([str(args.hirsel),'run',str(selection),'--replay',str(tape)] if args.engine=='interpreter'
                       else [str(binary),'--hirsel-replay',str(tape)])
        replay = subprocess.run([sys.executable,str(root/'capped'),'--timeout','60',*replay_argv],
            stdin=subprocess.DEVNULL,capture_output=True,env=env,timeout=75)
        (state/'replay.stdout').write_bytes(replay.stdout)
        (state/'replay.stderr').write_bytes(replay.stderr)
        receipt['replay'] = dict(argv=replay_argv,exit=replay.returncode,passed=replay.returncode==code and replay.stdout==stdout.read_bytes())
        (root/'child.unavailable').rename(root/'child.py')
        assert tape.read_bytes() == immutable
        (out/'receipts.json').write_text(json.dumps(receipts,indent=2))
        assert receipt['replay']['passed'], (identity, replay)
    assert source_seal == {p:digest(Path(p)) for p in source_seal}
    print(json.dumps(dict(engine=args.engine, selected=len(cases), completed=len(receipts),passed=sum(r['passed'] for r in receipts),replays=sum(r['replay']['passed'] for r in receipts),cleanup=True,
                          accounting=[dict(id=r['id'], creation_state=r['creation_state'], children=r['children']) for r in receipts])))


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--root',type=Path,required=True)
    parser.add_argument('--hirsel',type=Path,required=True)
    parser.add_argument('--case', action='append')
    parser.add_argument('--caps', action='store_true')
    parser.add_argument('--engine',choices=['interpreter','llvm','cranelift'],required=True)
    verify(parser.parse_args())
