"""B1 fence-only campaign; based on bench/multicore/suite.py:sample.
Three predeclared seeded interleaved rounds; observer overhead remains separate.
"""
import argparse
import csv
import gzip
import shutil
import hashlib
import json
import math
import os
from pathlib import Path
import random
import statistics
import subprocess
import time

CASES = ('fork_join', 'mutex', 'independent_files')
SIZES = (256, 512, 1024)
CPUS = set(range(8, 16))
SIBLINGS = set(range(24, 32))
SEED = 20261024


def digest(path):
    with Path(path).open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def snapshot():
    return {'proc_stat': Path('/proc/stat').read_text(),
            'loadavg': Path('/proc/loadavg').read_text(),
            'affinity': sorted(os.sched_getaffinity(0)),
            'cgroup': Path('/proc/self/cgroup').read_text()}


def sibling_load(before, after):
    def ticks(text):
        return {int(line.split()[0][3:]): list(map(int, line.split()[1:9]))
                for line in text.splitlines() if line.split()[0] in {f'cpu{c}' for c in SIBLINGS}}
    b, a = ticks(before['proc_stat']), ticks(after['proc_stat'])
    result = {}
    for cpu in sorted(SIBLINGS):
        delta = [x-y for x,y in zip(a[cpu], b[cpu])]
        total = sum(delta)
        result[str(cpu)] = {'total_ticks': total,
                            'busy_ticks': total-delta[3]-delta[4],
                            'busy_share': (total-delta[3]-delta[4])/total if total else None}
    return result


def sample(root, out, inventory, case, provider, record, observed, n, rnd):
    tag = f'{case}-{provider}-{record}-obs{observed}-n{n}-r{rnd}'
    scratch = out/'raw'/tag
    scratch.mkdir(parents=True, exist_ok=False)
    files = scratch/'files'; files.mkdir()
    tape = scratch/'recording'
    sidecar = scratch/'operations.jsonl'
    grants = {'io': {'stdout':'stdout.bytes@1', 'tasks':'tasks.free@1',
                    'dirs':[{'path':'','host':str(files),'access':'read_write'}]},
              'recording':'record' if record else 'none', 'placement':{'cores':8}}
    gp = scratch/'grants.json'; gp.write_text(json.dumps(grants))
    variant = 'observer' if observed else 'plain'
    binary = inventory['binaries'][variant][provider if provider=='tokio' else case]
    if provider == 'tokio':
        command = [binary, case, '8', str(n), str(files)]
    else:
        command = [binary, '--hirsel-grants', str(gp)]
        if record: command += ['--hirsel-record', str(tape)]
        command += [str(n)]
    env = {k:v for k,v in os.environ.items() if not k.startswith('HIRSEL_RT_')}
    env.pop('HIRSEL_OPERATION_LATENCY', None)
    env.update(LC_ALL='C', HIRSEL_JOBS='8', HIRSEL_MEMORY_LIMIT='2G')
    if observed: env['HIRSEL_OPERATION_LATENCY'] = str(sidecar)
    metrics = scratch/'resources.json'; counter = scratch/'perf.csv'
    # capped's --inside retains the outer timing.slice fence instead of moving
    # the measured child into a user.slice scope excluded from timing CPUs.
    # The outer transient service enforces MemoryMax=2G; RLIMIT_CPU is per child.
    args = [str(root/'tools/capped'), '--inside', '--timeout','60',
            '/usr/bin/time','-f','{"rss_kib":%M,"user_s":%U,"system_s":%S}', '-o',str(metrics),
            'prlimit','--cpu=60','--',inventory['perf_elf'],'stat','--no-big-num','-x',';',
            '-e','instructions:u,instructions,cycles,duration_time','-o',str(counter),'--',
            *command]
    (scratch/'command.json').write_text(json.dumps({'argv':args,'env':{k:v for k,v in env.items() if k.startswith('HIRSEL_') or k in ('PATH','LC_ALL','LD_LIBRARY_PATH')}}, indent=2)+'\n')
    print(tag, flush=True)
    before = snapshot()
    with (scratch/'stdout').open('wb') as stdout, (scratch/'stderr').open('wb') as stderr:
        result = subprocess.run(args, env=env, cwd=root, stdout=stdout, stderr=stderr)
    after = snapshot()
    row = dict(case=case,provider=provider,record=record,observed=observed,n=n,round=rnd,
               raw=str(scratch),exit=result.returncode,before=before,after=after,
               sibling_load=sibling_load(before,after))
    (scratch/'exit.json').write_text(json.dumps(row,indent=2)+'\n')
    assert result.returncode == 0, (tag,result.returncode)
    expected = f'{n*(n+1)//2 if case=="fork_join" else 8*n}\n'
    assert (scratch/'stdout').read_text() == expected, tag
    if case == 'independent_files':
        assert sorted(p.name for p in files.iterdir()) == [f'lane-{i}' for i in range(8)], tag
        for lane in range(8): assert (files/f'lane-{lane}').read_bytes() == b'x'*n, tag
    stats = json.loads(metrics.read_text())
    assert stats['rss_kib']*1024 <= 2147483648, tag
    assert stats['user_s']+stats['system_s'] <= 60, tag
    counts = {}
    for fields in csv.reader(counter.read_text().splitlines(), delimiter=';'):
        if len(fields)>2:
            try: counts[fields[2]] = float(fields[0])
            except ValueError: pass
    assert all(counts.get(k,0)>0 for k in ('instructions:u','instructions','cycles','duration_time')),tag
    row.update(resources=stats, counts=counts, elapsed_ns=counts['duration_time'])
    if record:
        assert tape.read_bytes().startswith(b'hirsel-free-tape 2\n'),tag
        row.update(tape_bytes=tape.stat().st_size,tape_sha256=digest(tape))
    if observed:
        paths = sorted(scratch.glob('operations.jsonl*'))
        assert paths, (tag,'missing sidecar')
        rows = []; headers=[]
        for path in paths:
            data = [json.loads(line) for line in path.read_text().splitlines()]
            headers.append(data[0]); rows.extend(data[1:])
        assert all(h['dropped']==0 for h in headers),(tag,headers)
        kind = {'fork_join':1,'mutex':2,'independent_files':3}[case]
        population = [r for r in rows if r['kind']==kind]
        statuses = {str(status):sum(r['status']==status for r in population) for status in range(5)}
        expected_count = n if case=='fork_join' else 8*n
        assert len({(r['task'],r['ordinal']) for r in rows})==len(rows),(tag,'duplicate operation token')
        assert len(population)==expected_count,(tag,len(population),expected_count)
        assert statuses['1']==expected_count,(tag,statuses)
        durations=sorted(r['end_ns']-r['start_ns'] for r in population)
        assert all(x>0 for x in durations),tag
        row['latency'] = dict(count=len(durations), statuses=statuses,
            all_kinds_statuses={f'{k}:{s}':sum(r['kind']==k and r['status']==s for r in rows)
                               for k in (1,2,3) for s in range(5)},
            p99_ns=durations[math.ceil(.99*len(durations))-1],
            p99_rank=math.ceil(.99*len(durations)), tail_count=len(durations)-math.ceil(.99*len(durations))+1)
    else:
        assert not list(scratch.glob('operations.jsonl*')),tag
    row['binary_sha256'] = digest(binary)
    (scratch/'binding.json').write_text(json.dumps(dict(binary=binary, binary_sha256=row['binary_sha256'],
        command_sha256=digest(scratch/'command.json'), grants_sha256=digest(gp),
        tape_sha256=row.get('tape_sha256'),
        sidecar_sha256={p.name:digest(p) for p in scratch.glob('operations.jsonl*')}),indent=2)+'\n')
    # Retain exact raw bytes compactly, outside the measured child.
    for path in [tape, *scratch.glob('operations.jsonl*')]:
        if path.is_file():
            with path.open('rb') as source, gzip.open(str(path)+'.gz','wb') as target:
                shutil.copyfileobj(source,target)
            path.unlink()
    return row


def analyze(samples):
    def group(case,n,provider,record,observed):
        rows=[r for r in samples if (r['case'],r['n'],r['provider'],r['record'],r['observed']) ==
              (case,n,provider,record,observed)]
        assert sorted(r['round'] for r in rows)==[0,1,2]
        return rows
    answer=[]
    for case in CASES:
        for n in SIZES:
            tokio=group(case,n,'tokio',False,False)
            off=group(case,n,'free',False,False); on=group(case,n,'free',True,False)
            obsoff=group(case,n,'free',False,True); obson=group(case,n,'free',True,True)
            def wall(rows): return statistics.median(r['elapsed_ns'] for r in rows)
            def p99(rows): return statistics.median(r['latency']['p99_ns'] for r in rows)
            a,b,c=wall(tokio)/wall(off),wall(off)/wall(on),p99(obson)/p99(obsoff)
            answer.append(dict(case=case,n=n,off_vs_tokio=a,record_vs_off=b,p99_record_vs_off=c,
                p99_off_ns=p99(obsoff),p99_record_ns=p99(obson),
                observer_wall_overhead_off=wall(obsoff)/wall(off),
                observer_wall_overhead_record=wall(obson)/wall(on),
                off_instructions_vs_tokio=statistics.median(r['counts']['instructions:u'] for r in off)/statistics.median(r['counts']['instructions:u'] for r in tokio),
                floors={'off':a>=.8,'record':b>=.9,'p99':c<=1.2}))
    return answer


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--root',type=Path,required=True)
    parser.add_argument('--out',type=Path,required=True)
    parser.add_argument('--inventory',type=Path,required=True)
    parser.add_argument('--smoke',action='store_true')
    args=parser.parse_args()
    inventory=json.loads(args.inventory.read_text())
    for path,sha in inventory.get('sha256',{}).items(): assert digest(path)==sha,path
    if not args.smoke:
        assert set(os.sched_getaffinity(0)) == CPUS
        assert float(Path('/proc/loadavg').read_text().split()[0]) < os.cpu_count()
        for cpu in CPUS:
            siblings=set(map(int,Path(f'/sys/devices/system/cpu/cpu{cpu}/topology/thread_siblings_list').read_text().strip().split(',')))
            assert siblings & CPUS == {cpu}
            assert siblings-{cpu} <= SIBLINGS
    session={'seed':SEED,'rounds':3,'before':snapshot(),'samples':[],'status':'running'}
    path=args.out/'session.json'
    def save(): path.write_text(json.dumps(session,indent=2)+'\n')
    cells=[(case,p,r,o,n) for case in CASES for n in SIZES for p,r,o in
           [('tokio',False,False),('free',False,False),('free',True,False),
            ('free',False,True),('free',True,True)]]
    if args.smoke: cells=[(case,p,r,o,32) for case in CASES for p,r,o in
                          [('tokio',False,False),('free',False,False),('free',True,False),('free',False,True),('free',True,True)]]
    rng=random.Random(SEED)
    try:
        for rnd in range(1 if args.smoke else 3):
            rng.shuffle(cells)
            for cell in cells:
                session['samples'].append(sample(args.root,args.out,inventory,*cell,rnd));save()
        session['status']='complete'
        if not args.smoke: session['analysis']=analyze(session['samples'])
    except BaseException as error:
        session.update(status='failed',error=repr(error));raise
    finally:
        session['after']=snapshot();session['sibling_load']=sibling_load(session['before'],session['after']);save()

if __name__=='__main__': main()
