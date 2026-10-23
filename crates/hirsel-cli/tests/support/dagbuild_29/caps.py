# Generated boundary vectors stay small on disk; the harness expands exact values.
def boundary_cases():
    from oracle import task,record
    import json
    cases=[]
    def add(identity,tasks,exit=0,error=None,files=None,jobs=2,raw_size=None):
        files=files or {};manifest=dict(tasks=tasks,jobs=jobs,fail_fast=False)
        prior=None
        if not error:
            state=record(tasks,files)
            rows=[dict(name=t['name'],status='built',reason='no_record',digest_result=r['record']['digest_result']) for t,r in zip(tasks,state['tasks'])]
            rows.append(dict(tasks=len(tasks),expired=False,failed=False))
        else:state=None;rows=[]
        cases.append(dict(id=identity,manifest=manifest,files=files,prior=prior,args=['build','manifest.json'],exit=exit,denied=None,rows=rows,stderr=json.dumps(dict(kind=error),separators=(',',':'))+'\n' if error else '',spawned=0 if error else len(tasks),state=state,raw_size=raw_size))
    for n in [32,33]:add('nodes-'+str(n),[task(str(i)) for i in range(n)],0 if n==32 else 2,None if n==32 else 'node_limit')
    for n in [128,129]:add('edges-'+str(n),[task('a'),task('b',['a']*n)],0 if n==128 else 2,None if n==128 else 'edge_limit')
    for n in [64,65]:add('artifacts-'+str(n),[task('a',inputs=['in']*n)],0 if n==64 else 2,None if n==64 else 'artifact_limit',{'in':''})
    for n in [4096,4097]:
        t=task('a');t['argv']=['/fixture/child','x'*(n-len('/fixture/child')-2)]
        add('argv-'+str(n),[t],0 if n==4096 else 2,None if n==4096 else 'argv_bytes')
    for n in [65536,65537]:add('manifest-'+str(n),[],0 if n==65536 else 3,None if n==65536 else 'manifest_bytes',raw_size=n)
    for n in [1048576,1048577]:add('bytes-'+str(n),[task('a',inputs=['in'])],0 if n==1048576 else 3,None if n==1048576 else 'artifact_bytes',{'in':'x'*n})
    for n in [0,1,4,5]:add('jobs-'+str(n),[],0 if 1<=n<=4 else 2,None if 1<=n<=4 else 'jobs',jobs=n)
    return cases
