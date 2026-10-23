"""Production Ordered full-app launches, immutable tapes and real-file effects.
Each measured process has independent 60 CPU-s/2GiB caps. Builds are separate.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess


def main(a):
    root = a.root.resolve()
    env = dict(os.environ, HIRSEL_MEMORY_LIMIT='2G', HIRSEL_JOBS='4')
    for key in list(env):
        if key.startswith('HIRSEL_RT_'):
            env.pop(key)
    receipts = []

    def save():
        (root/'receipts.json').write_text(json.dumps(receipts, indent=2)+'\n')

    def call(argv, name, setup=False):
        wrapped = ['python3', str(root/'capped'), '--timeout', '300' if setup else '60']
        if not setup:
            wrapped += ['prlimit', '--cpu=60:61']
        done = subprocess.run(wrapped+list(map(str, argv)), stdin=subprocess.DEVNULL,
                              capture_output=True, env=env)
        (root/(name+'.stdout')).write_bytes(done.stdout)
        (root/(name+'.stderr')).write_bytes(done.stderr)
        stderr = b''.join(line for line in done.stderr.splitlines(keepends=True)
                          if not line.startswith(b'capped: '))
        return done.returncode, done.stdout, stderr

    if os.environ.get('OBJECT_STORE_30_BINARIES'):
        a.reuse = Path(os.environ['OBJECT_STORE_30_BINARIES'])
    binaries = {}
    for backend in ['llvm', 'cranelift']:
        binary = (a.reuse or root)/('app-'+backend)
        if not a.reuse:
            status, out, err = call([a.hirsel, 'build', root/'hirsel.json', '--backend',
                                    backend, '-o', binary], backend+'-build', True)
            assert status == 0, (backend, status, out, err)
        binaries[backend] = binary
    tools = {str(path.resolve()): hashlib.sha256(path.read_bytes()).hexdigest()
             for path in [a.hirsel, *binaries.values(), root/'native.py', root/'oracle.py', root/'capped']}
    (root/'native-tools-before.json').write_text(json.dumps(tools, indent=2)+'\n')
    selected = json.loads((root/'cases.json').read_text())
    # Pending is a public simulation-only provider, not a production file grant.
    selected = [c for c in selected if not c.get('pending')]
    filter_ids = os.environ.get('OBJECT_STORE_30_CASE')
    if not filter_ids and not os.environ.get('OBJECT_STORE_30_BOUNDARIES'):
        selected = [c for c in selected if not c.get('boundary')]
    if filter_ids:
        selected = [c for c in selected if c['id'] in filter_ids.split(',')]
    assert selected
    for backend, binary in binaries.items():
        for case in selected:
            name = backend+'-'+case['id']
            world = root/'world'
            for directory in ['source', 'objects', 'roots']:
                (world/directory).mkdir(parents=True)
            for path, value in case['files'].items():
                path = world/path
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_bytes(bytes.fromhex(value))
            for path, target in case.get('links', {}).items():
                path = world/path
                path.parent.mkdir(parents=True, exist_ok=True)
                path.symlink_to(target)
            grants = dict(args=case['args'], io=dict(stdout='stdout.bytes@1',
                          stderr='stderr.bytes@1', tasks='tasks.cooperative@1', dirs=[
                              dict(path=n, host=str(world/n), access='read' if n == 'source' or
                                   n == 'roots' and case.get('readonly') else 'read_write')
                              for n in ['objects', 'roots', 'source']]),
                          placement=dict(cores=1, commit='shared', reorder='off'))
            if case.get('signal_watch'):
                grants['io']['signals']=dict(provider='signals.host@1',kinds=['interrupt'])
            grant = root/(name+'-grants.json')
            grant.write_text(json.dumps(grants))
            tape = root/(name+'.tape')
            argv = [binary, '--hirsel-record', tape, '--hirsel-grants', grant, *case['args']]
            row = dict(id=name, state='started', argv=list(map(str, argv)), tape=tape.name,
                       steps=case['steps'], stdout=case['stdout'], stderr=case['stderr'],
                       exit=case['exit'])
            receipts.append(row)
            save()
            status, out, err = call(argv, name)
            row.update(state='completed', actual_exit=status, actual_stdout=out.hex(), actual_stderr=err.hex())
            save()
            assert (status, out, err) == (case['exit'], bytes.fromhex(case['stdout']), bytes.fromhex(case['stderr'])), (name, status, out, err)
            actual = {str(p.relative_to(world)): p.read_bytes().hex() for p in world.rglob('*') if p.is_file() and not p.is_symlink()}
            assert actual == case['after'], (name, actual, case['after'])
            original = tape.read_bytes()
            shutil.rmtree(world)
            grant.unlink()
            for target, executable in binaries.items():
                replay = call([executable, '--hirsel-replay', tape], name+'-replay-'+target)
                assert replay == (status, out, err), (name, target, replay)
                assert not world.exists()
                assert tape.read_bytes() == original
            row.update(passed=True, tape_sha256=hashlib.sha256(original).hexdigest(), native_replays=2)
            save()
            print(json.dumps(dict(id=name, passed=True)), flush=True)
    assert all(row.get('passed') for row in receipts)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--hirsel', type=Path, required=True)
    parser.add_argument('--root', type=Path, required=True)
    parser.add_argument('--reuse', type=Path)
    main(parser.parse_args())
