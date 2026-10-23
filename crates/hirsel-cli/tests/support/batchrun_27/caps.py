#!/usr/bin/env python3
"""Full admission vectors on native backends; no model substituted for the app."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess


def main(args):
    root=args.root
    env={k:v for k,v in os.environ.items() if not k.startswith('HIRSEL_RT_')}
    env.update(HIRSEL_DAEMON='off',HIRSEL_RESULTS='none@1',HIRSEL_JOBS='1',HIRSEL_MEMORY_LIMIT='2G')
    output=root/'caps-evidence';output.mkdir()
    selected=json.loads((root/('cases.json' if args.workflows else 'caps.json')).read_text())
    if args.workflows:
        selected=[case for case in selected if case['id'] not in ['child-deadline','global-deadline']]
    rows=[]
    for backend in ['llvm','cranelift']:
        binary=output/backend
        build=[str(args.hirsel),'build',str(root/'hirsel.json'),'--backend',backend,'-o',str(binary)]
        done=subprocess.run(build,env=env,capture_output=True,timeout=300)
        (output/(backend+'-build.stdout')).write_bytes(done.stdout)
        (output/(backend+'-build.stderr')).write_bytes(done.stderr)
        assert done.returncode==0,(backend,done)
        for case in selected:
            identity=case['id']
            d=output/(backend+'-'+identity);d.mkdir();data=d/'data';data.mkdir()
            source=data/'manifest.json';source.write_text(json.dumps(case['manifest']))
            valid=case['exit']!=2
            children=[]
            if valid and identity != 'absent':
                for i,command in enumerate(case['manifest']['commands']):
                    mode=case['modes'][i]
                    steps=([dict(print=[79,79,79]),dict(print_err=[69,69,69])] if mode=='pipes' else [])
                    steps.append(dict(exit=dict(signaled=15) if mode=='signal' else dict(exited=7 if mode=='fail' else 0)))
                    children.append(dict(program='/fixture/child',args=command['argv'][1:],steps=steps))
            grants=dict(args=['manifest.json'],io=dict(stdout='stdout.bytes@1',stderr='stderr.bytes@1',tasks='tasks.cooperative@1',monotonic_clock='clock.monotonic.ns@1',dirs=[dict(path='fixtures',host=str(data),access='read')],process=dict(provider='process.script@1',rules=[] if case['refused'] else [dict(program='/fixture/child')],runs=[],children=children)),placement=dict(cores=1,commit='shared',reorder='auto'))
            grant=d/'grants.json';grant.write_text(json.dumps(grants))
            tape=d/'record.tape'
            expected=dict(exit=case['exit'],rows=case['rows'],stderr=case['stderr'])
            (d/'expected.json').write_text(json.dumps(expected))
            argv=[str(binary),'--hirsel-record',str(tape),'--hirsel-grants',str(grant),'manifest.json']
            receipt=dict(id=identity,engine=backend,argv=argv,state='started',input_sha256=hashlib.sha256(source.read_bytes()).hexdigest(),expected=expected)
            rows.append(receipt);(output/'receipts.json').write_text(json.dumps(rows,indent=2))
            done=subprocess.run(argv,stdin=subprocess.DEVNULL,capture_output=True,env=env,timeout=30)
            (d/'stdout').write_bytes(done.stdout);(d/'stderr').write_bytes(done.stderr)
            actual=dict(exit=done.returncode,rows=[json.loads(line) for line in done.stdout.splitlines()],stderr=done.stderr.decode())
            receipt.update(state='completed',actual=actual,passed=actual==expected)
            (output/'receipts.json').write_text(json.dumps(rows,indent=2))
            assert actual==expected,(identity,backend,actual,expected)
    print(json.dumps(dict(selected=len(rows),completed=len(rows),passed=sum(row['passed'] for row in rows))))


if __name__=='__main__':
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--workflows',action='store_true')
    parser.add_argument('--root',type=Path,required=True)
    parser.add_argument('--hirsel',type=Path,required=True)
    main(parser.parse_args())
