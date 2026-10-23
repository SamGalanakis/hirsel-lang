"""Run the shipped app against frozen sequential-oracle bytes, then replay without files."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys


def clean_env():
    env = {k: v for k, v in os.environ.items() if not k.startswith('HIRSEL_RT_')}
    env.update(HIRSEL_DAEMON='off', HIRSEL_RESULTS='none@1', HIRSEL_JOBS='1',
               HIRSEL_MEMORY_LIMIT='2G', HIRSEL_TIMING='off')
    return env


def main(args):
    root = args.root
    out = root / 'evidence'
    out.mkdir()
    data = json.loads((root / 'cases.json').read_bytes())
    env = clean_env()
    (out / 'environment.json').write_text(json.dumps({k:v for k,v in env.items() if k.startswith('HIRSEL_')}, indent=2)+'\n')
    binary = out / args.backend
    argv = [str(args.hirsel), 'build', str(root/'hirsel.json'), '--backend', args.backend, '-o', str(binary)]
    built = subprocess.run(argv, env=env, capture_output=True, timeout=300)
    (out/'build.stdout').write_bytes(built.stdout)
    (out/'build.stderr').write_bytes(built.stderr)
    (out/'build.json').write_text(json.dumps(dict(argv=argv,status=built.returncode)))
    assert built.returncode == 0, built
    fixture = root / 'fixtures'
    fixture.mkdir()
    for path, text in data['files'].items():
        (fixture/path).parent.mkdir(parents=True,exist_ok=True)
        (fixture/path).write_bytes(text.encode())
    before = {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in fixture.rglob('*') if p.is_file()}
    receipts = []
    def run(id, argv, row):
        result_file = out / (id+'.result.json')
        command = [sys.executable, str(root/'capped'), '--timeout', '60', sys.executable,
                   str(root/'verify.py'), '--child', str(result_file), '--', *map(str,argv)]
        receipt = dict(id=id,engine=args.backend,argv=command,started=1,completed=0,passed=0)
        receipts.append(receipt)
        (out/'receipts.json').write_text(json.dumps(receipts,indent=2)+'\n')
        print('PARHASH_27_STARTED '+json.dumps(receipt),flush=True)
        done = subprocess.run(command,env=env,capture_output=True,timeout=90)
        (out/(id+'.resources')).write_bytes(done.stderr)
        assert result_file.exists(), (id,done)
        actual = json.loads(result_file.read_bytes())
        receipt.update(completed=1,status=actual['status'])
        receipt['passed'] = int((done.returncode,actual['status'],actual['stdout'],actual['stderr']) ==
                               (row['status'],row['status'],row['stdout'],row['stderr']))
        (out/'receipts.json').write_text(json.dumps(receipts,indent=2)+'\n')
        assert receipt['passed'], (id,actual,row,done)
        print('PARHASH_27_COMPLETED '+json.dumps(receipt),flush=True)
    # One ordinary production recording of every workflow. No worker test selectors.
    for row in data['cases']:
        id = row['id']
        grants = dict(io=dict(stdout='stdout.bytes@1',stderr='stderr.bytes@1',tasks='tasks.cooperative@1',
                      dirs=[dict(path='fixtures',host=str(fixture),access='read')]),args=row['args'],
                      placement=dict(cores=1,commit='shared',reorder='auto',metrics=str(out/(id+'.metrics'))))
        grants_file = out/(id+'.grants.json')
        grants_file.write_text(json.dumps(grants,indent=2)+'\n')
        argv = [binary,'--hirsel-record',out/(id+'.tape'),
                '--hirsel-grants',grants_file,*row['args']]
        run(id,argv,row)
        assert {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in fixture.rglob('*') if p.is_file()} == before
    # Effects unavailable: neither an initial host directory nor its files exist here.
    fixture.rename(root/'unavailable')
    for row in data['cases']:
        id = row['id']
        tape = out/(id+'.tape')
        seal = hashlib.sha256(tape.read_bytes()).hexdigest()
        run(id+'-native-replay',[binary,'--hirsel-replay',tape],row)
        run(id+'-reference-replay',[args.hirsel,'run',root/'hirsel.json','--replay',tape],row)
        assert hashlib.sha256(tape.read_bytes()).hexdigest() == seal
        assert not fixture.exists()
    print(json.dumps(dict(backend=args.backend,cases=len(data['cases']),observations=len(receipts),cleanup=True)))


if __name__ == '__main__':
    if len(sys.argv)>1 and sys.argv[1]=='--child':
        done = subprocess.run(sys.argv[4:],capture_output=True)
        Path(sys.argv[2]).write_text(json.dumps(dict(status=done.returncode,stdout=done.stdout.decode(),stderr=done.stderr.decode()))+'\n')
        sys.exit(done.returncode)
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--root',type=Path,required=True)
    parser.add_argument('--hirsel',type=Path,required=True)
    parser.add_argument('--backend',choices=['llvm','cranelift'],required=True)
    main(parser.parse_args())
