"""Run frozen filemanifest inputs; no matching, walk or hash oracle is recomputed."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys


def verify(args):
    root, output = args.root, args.root / 'evidence'
    output.mkdir()
    manifest = json.loads((root / 'manifest.json').read_bytes())
    env = os.environ | dict(HIRSEL_DAEMON='off', HIRSEL_RESULTS='none@1',
                            HIRSEL_JOBS='1', HIRSEL_MEMORY_LIMIT='2G', HIRSEL_TIMING='off')
    receipts = []
    child = root / 'child.py'
    child.write_text('''import pathlib,resource,subprocess,sys
resource.setrlimit(resource.RLIMIT_CPU,(60,60))
p=subprocess.run(sys.argv[3:],stdin=subprocess.DEVNULL,capture_output=True)
pathlib.Path(sys.argv[1]).write_bytes(p.stdout)
pathlib.Path(sys.argv[2]).write_bytes(p.stderr)
sys.exit(p.returncode)
''')
    selection = root / 'hirsel.json'
    binary = output / args.backend
    command = [str(args.hirsel), 'build', str(selection), '--backend', args.backend, '-o', str(binary)]
    setup = subprocess.run(command, capture_output=True, env=env, timeout=300)
    (output/'build.stdout').write_bytes(setup.stdout)
    (output/'build.stderr').write_bytes(setup.stderr)
    assert (setup.returncode, setup.stdout, setup.stderr) == (0, b'', b''), setup
    for row in manifest['cases']:
        if row['id'].startswith('cap-') != (args.partition == 'caps'):
            continue
        id = row['id']
        for suffix, key in [('input', 'input_sha256'), ('expected', 'expected_sha256')]:
            assert hashlib.sha256((root/f'{id}.{suffix}.json').read_bytes()).hexdigest() == row[key]
        data = json.loads((root/f'{id}.input.json').read_bytes())
        expected = json.loads((root/f'{id}.expected.json').read_bytes())
        tree = output/id
        tree.mkdir()
        for name in data['directories']:
            (tree/name).mkdir(parents=True, exist_ok=True)
        for name, text in data['files'].items():
            path = tree/name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(text.encode())
        for name, target in data['links'].items():
            (tree/name).symlink_to(target)
        if data['directory_error']:
            # Existing walk_stream host test uses the same portable read fault.
            bad = os.fsencode(tree/data['directory_error']) + b'/\xff'
            with open(bad, 'wb') as file:
                file.write(b'')
        def snapshot():
            values = {}
            for directory, dirs, files in os.walk(os.fsencode(tree), followlinks=False):
                for name in dirs+files:
                    path = directory+b'/'+name
                    stat = os.lstat(path)
                    values[path] = (stat.st_mode, os.readlink(path) if os.path.islink(path) else
                                    hashlib.sha256(Path(os.fsdecode(path)).read_bytes()).digest() if os.path.isfile(path) else None)
            return values
        before = snapshot()
        grants = output/f'{id}.grants.json'
        grants.write_text(json.dumps(dict(io=dict(stdout='stdout.bytes@1', stderr='stderr.bytes@1',
            dirs=[dict(path='fixtures', host=str(tree), access='read')]), args=['scan','fixtures',data['pattern']])))
        stdout, stderr = output/f'{id}.stdout', output/f'{id}.stderr'
        tape = output/f'{id}.tape'
        command = [sys.executable, str(root/'capped'), '--timeout', '60', sys.executable,
                   str(child), str(stdout), str(stderr), str(binary), '--hirsel-record',
                   str(tape), '--hirsel-grants', str(grants), 'scan', 'fixtures', data['pattern']]
        receipts.append(dict(id=id,engine=args.backend,input_sha256=row['input_sha256'],
                             expected_sha256=row['expected_sha256'],command=command,state='started'))
        (output/'receipts.json').write_text(json.dumps(receipts,indent=2)+'\n')
        print('FILEMANIFEST_STREAM_25_NATIVE_START '+json.dumps(receipts[-1]), flush=True)
        done = subprocess.run(command, capture_output=True, env=env, timeout=120)
        (output/f'{id}.resources').write_bytes(done.stderr)
        actual = done.returncode, stdout.read_bytes(), stderr.read_bytes()
        receipts[-1].update(state='completed',exit=done.returncode,passed=actual ==
            (expected['status'],expected['stdout'].encode(),expected['stderr'].encode()))
        (output/'receipts.json').write_text(json.dumps(receipts,indent=2)+'\n')
        print('FILEMANIFEST_STREAM_25_NATIVE_END '+json.dumps(receipts[-1]), flush=True)
        assert receipts[-1]['passed'], (id,args.backend,actual,expected)
        assert snapshot() == before, (id,'read-only snapshot changed')
    assert len(receipts) == 4
    print(json.dumps(dict(engine=args.backend,selected=4,started=4,completed=4,passed=4,failed=0,unrun=0)))


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--root', type=Path, required=True)
    parser.add_argument('--hirsel', type=Path, required=True)
    parser.add_argument('--backend', choices=['llvm','cranelift'], required=True)
    parser.add_argument('--partition', choices=['caps','lazy'], required=True)
    verify(parser.parse_args())
