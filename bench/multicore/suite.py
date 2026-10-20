"""B1 measurement and acceptance. Missing evidence never passes a floor."""
import hashlib
import json
import math
import os
from pathlib import Path
import shlex
import statistics
import subprocess
import sys
import tempfile
HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
sys.path.insert(0, str(ROOT/'bench/micro'))
from measure import perf_counts, siblings, busy_share, bootstrap_ratio  # noqa: E402

MODES = ('off', 'record', 'durable')

def digest(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def quantity(formula, n):
    return {'8*n':8*n,'24*n':24*n,'56*n':56*n,'n':n,'3*n':3*n,
            '4*n':4*n,'6*n':6*n,'n*(n+1)//2':n*(n+1)//2}[formula]


def cpu_snapshot(text, cpus):
    result = {}
    for line in text.splitlines():
        fields = line.split()
        if fields and fields[0] in {f'cpu{cpu}' for cpu in cpus}:
            ticks = list(map(int, fields[1:9]))
            result[fields[0]] = {'total':sum(ticks),'idle':ticks[3]+ticks[4]}
    return result


def sample(participant, case, spec, width, mode, cpus, timeout, placement=False, budget=None):
    """One process per capped call. perf/time exclude the capped queue wait."""
    with tempfile.TemporaryDirectory(prefix='mc-bench-') as directory:
        scratch = Path(directory)
        root = scratch/'files'; root.mkdir()
        recording = scratch/'recording'
        metrics = scratch/'metrics.json'
        grants = {'io':{'stdout':'stdout.bytes@1','tasks':participant['provider'],
            'dirs':[{'path':'','host':str(root),'access':'read_write'}]},
            'recording':{'off':'none','record':'record','durable':'durable'}[mode]}
        if placement:
            grants['placement'] = {'cores':width, 'metrics':str(metrics)}
        if budget:
            grants['budgets'] = budget
        grants_path = scratch/'grants.json'
        grants_path.write_text(json.dumps(grants))
        replacements = {'width':str(width),'iterations':str(spec['iterations']),
            'root':str(root),'grants':str(grants_path),'recording':str(recording),'metrics':str(metrics),'mode':mode}
        command = [arg.format_map(replacements) for arg in participant['commands'][case]]
        if participant['provider'].startswith('tasks.') and mode != 'off':
            command[1:1] = ['--hirsel-record', str(recording)]
        env = dict(os.environ, LC_ALL='C', HIRSEL_JOBS=str(width))
        # Ambient harness flags must never turn off into an accidental recording.
        for key in list(env):
            if key.startswith('HIRSEL_RT_'): env.pop(key)
        env.update({k:v.format_map(replacements) for k,v in participant.get('env',{}).items()})
        before, after = scratch/'before.json', scratch/'after.json'
        counter, resource = scratch/'perf.csv', scratch/'time.json'
        perf = ['taskset','-c',','.join(map(str,cpus[:width])), 'perf','stat','--no-big-num','-x',';',
            '-e','cycles,instructions,task-clock,duration_time','-o',str(counter),
            '--pre',shlex.join([str(HERE/'snapshot'),str(before)]),
            '--post',shlex.join([str(HERE/'snapshot'),str(after)]),'--',
            '/usr/bin/time','-f','{"rss_kib":%M,"user_s":%U,"system_s":%S}','-o',str(resource), *command]
        proc = subprocess.run([str(ROOT/'tools/capped'),'--timeout',str(timeout),*perf],
            env=env,cwd=participant.get('cwd',str(ROOT)),capture_output=True,text=True)
        if proc.returncode:
            raise ValueError(f'{participant["name"]}/{case}/{width}/{mode}: exit {proc.returncode}: {proc.stderr[-3000:]}')
        expected = str(quantity(spec['checksum'],spec['iterations'])) + '\n'
        if proc.stdout != expected:
            raise ValueError(f'checksum mismatch: {proc.stdout!r} != {expected!r}')
        text = counter.read_text()
        counts, running = perf_counts(text, ('cycles','instructions','task-clock','duration_time'))
        elapsed = counts['duration_time']/1e9
        if not math.isfinite(elapsed) or elapsed <= 0: raise ValueError('invalid perf elapsed')
        own = {}
        if metrics.exists():
            # External wrappers supply a single final schema-1 metrics object.
            own = json.loads(metrics.read_text())
            if own.get('schema') != 1: raise ValueError('unknown telemetry schema')
            if own.get('width') != width: raise ValueError('telemetry width differs from requested placement')
        stats = json.loads(resource.read_text())
        b, a = json.loads(before.read_text()), json.loads(after.read_text())
        sibling_cpus = sorted({s for c in cpus[:width] for s in siblings(c)} - set(cpus[:width]))
        throttled = None if b['throttled_usec'] is None or a['throttled_usec'] is None else a['throttled_usec']-b['throttled_usec']
        return {'participant':participant['name'],'provider':participant['provider'],'case':case,'width':width,'mode':mode,
            'elapsed_ns':elapsed*1e9,'operations':quantity(spec['operations'],spec['iterations']),
            'counts':counts,'running_percent':running,'rss_bytes':stats['rss_kib']*1024,
            'throttled_usec':throttled,'cgroup':a['cgroup'],
            'sibling_busy_share':busy_share(cpu_snapshot(b['proc_stat'],sibling_cpus),cpu_snapshot(a['proc_stat'],sibling_cpus)),
            'tape_bytes':sum(p.stat().st_size for p in scratch.rglob('*') if p.is_file() and (p == recording or recording in p.parents)),
            'metrics':own.get('metrics',{}),'stderr':proc.stderr}


def median(samples, key):
    values = [s[key] for s in samples if s.get(key) is not None]
    return statistics.median(values) if len(values) == len(samples) and values else None


def metric(samples, key):
    values = [s['metrics'].get(key) for s in samples]
    if not values or any(v is None for v in values): return None
    if any(not isinstance(v,(int,float)) or not math.isfinite(v) or v < 0 for v in values):
        raise ValueError(f'invalid metric {key}')
    return statistics.median(values)


def groups(data):
    result = {}
    for sample in data['samples']:
        result.setdefault((sample['provider'],sample['case'],sample['width'],sample['mode']),[]).append(sample)
    for value in result.values():
        value.sort(key=lambda s:s['round'])
        rounds = [s['round'] for s in value]
        if rounds != list(range(data['rounds'])): value.clear()
    return {key:value for key,value in result.items() if value}


def assess(value, threshold, lower=True):
    if value is None: return 'dependency-owned'
    return 'pass' if (value >= threshold if lower else value <= threshold) else 'fail'


def analyze(data):
    for sample in data['samples']:
        for key,value in sample['metrics'].items():
            if value is not None and (not isinstance(value,(int,float)) or not math.isfinite(value) or value<0):
                raise ValueError(f'invalid metric {key}')
    cells = groups(data)
    spec = data['manifest']; thresholds = spec['thresholds']; rows = []; headroom = []
    for case in spec['workloads']:
        for width in spec['widths']:
            tokio = cells.get(('tokio',case,width,'off'),[])
            off = cells.get(('tasks.free@1',case,width,'off'),[])
            record = cells.get(('tasks.free@1',case,width,'record'),[])
            durable = cells.get(('tasks.free@1',case,width,'durable'),[])
            coop = cells.get(('tasks.cooperative@1',case,1,'durable'),[])
            def throughput(a,b):
                return median(b,'elapsed_ns')/median(a,'elapsed_ns') if a and b else None
            live = throughput(off,tokio); overhead = throughput(record,off); durability = throughput(durable,coop)
            p99a, p99b = metric(record,'latency_p99_ns'),metric(off,'latency_p99_ns')
            p99 = p99a/p99b if p99a is not None and p99b else None
            size = metric(record,'compressed_bytes_per_contended_event')
            instruction_ratio = None
            if off and tokio:
                instruction_ratio = statistics.median(s['counts']['instructions'] for s in off)/statistics.median(s['counts']['instructions'] for s in tokio)
            rows.append({'case':case,'width':width,'free_vs_tokio':live,'instructions_free_vs_tokio':instruction_ratio,
                'record_vs_off':overhead,'p99_record_vs_off':p99,'compressed_bytes_per_contended_event':size,
                'durable_vs_cooperative_width1':durability,
                'gates':{'live':assess(live,thresholds['free_vs_tokio']),
                    'record':assess(overhead,thresholds['record_vs_off']),
                    'p99':assess(p99,thresholds['record_p99_vs_off'],False),
                    'bytes':assess(size,thresholds['bytes_per_contended_event'],False),
                    'durable':assess(durability,thresholds['durable_vs_cooperative_width1'])}})
            for row in spec['headroom']:
                if case not in row['workloads']: continue
                for key in row['metrics']:
                    left,right = metric(record if row['row']=='recording' else off,key),metric(off if row['row']=='recording' else tokio,key)
                    headroom.append({'row':row['row'],'case':case,'width':width,'metric':key,
                        'free':left,'tokio':right,'gap':left-right if left is not None and right is not None else None,
                        'comparison':'record-vs-off' if row['row']=='recording' else 'free-vs-tokio',
                        'status':'measured' if left is not None and right is not None else 'dependency-owned'})
    complete = not data.get('partial',False) and data['rounds'] >= 10 and len(data.get('errors',[])) == 0 and all(v == 'pass' for row in rows for v in row['gates'].values()) and all(r['status']=='measured' for r in headroom)
    return {'status':'pass' if complete else 'fail' if any(v=='fail' for row in rows for v in row['gates'].values()) or data.get('errors') else 'dependency-owned',
            'floors':rows,'headroom':headroom}


def display(value):
    return '—' if value is None else f'{value:.4g}'


def report(data):
    result = analyze(data)
    lines = ['# B1 multicore benchmark session','',f"Status: **{result['status']}**. Instruction ratios are the verdict; time ratios apply only within this session.",
        '',f"Seed {data['seed']}; {data['rounds']} interleaved rounds; CPUs {data['cpus']}. Process-lifetime perf counts include launch and teardown. p99 requires operation latency telemetry; it is never inferred from ten process timings.",
        '', '## Baselines', '', '| Provider | Workload | Width | Mode | ns/op | Instructions/op | Peak RSS bytes | Throttled µs |', '|---|---|---:|---|---:|---:|---:|---:|']
    for (provider,case,width,mode),samples in sorted(groups(data).items()):
        lines.append(f"| {provider} | {case} | {width} | {mode} | {display(statistics.median(s['elapsed_ns']/s['operations'] for s in samples))} | {display(statistics.median(s['counts']['instructions']/s['operations'] for s in samples))} | {display(median(samples,'rss_bytes'))} | {display(median(samples,'throttled_usec'))} |")
    lines += ['', '## Width-1 cooperative/tokio gaps', '', '| Workload | Throughput coop/tokio | Instructions coop/tokio | Time coop/tokio CI95 |', '|---|---:|---:|---|']
    for case in data['manifest']['workloads']:
        cells = groups(data)
        coop = cells.get(('tasks.cooperative@1',case,1,'off'),[])
        tokio = cells.get(('tokio',case,1,'off'),[])
        t = median(tokio,'elapsed_ns')/median(coop,'elapsed_ns') if coop and tokio else None
        instructions = statistics.median(s['counts']['instructions'] for s in coop)/statistics.median(s['counts']['instructions'] for s in tokio) if coop and tokio else None
        ci = bootstrap_ratio([s['elapsed_ns'] for s in coop],[s['elapsed_ns'] for s in tokio],data['seed'],draws=1000) if coop and tokio else None
        lines.append(f"| {case} | {display(t)} | {display(instructions)} | {ci if ci else '—'} |")
    lines += ['', '## Cooperative width-1 recording overhead', '', '| Workload | Record/off throughput | Instructions record/off | Durable/off throughput | Tape bytes (record) |', '|---|---:|---:|---:|---:|']
    cells = groups(data)
    for case in data['manifest']['workloads']:
        off = cells.get(('tasks.cooperative@1',case,1,'off'),[])
        rec = cells.get(('tasks.cooperative@1',case,1,'record'),[])
        dur = cells.get(('tasks.cooperative@1',case,1,'durable'),[])
        ratio = median(off,'elapsed_ns')/median(rec,'elapsed_ns') if off and rec else None
        dratio = median(off,'elapsed_ns')/median(dur,'elapsed_ns') if off and dur else None
        iratio = statistics.median(s['counts']['instructions'] for s in rec)/statistics.median(s['counts']['instructions'] for s in off) if off and rec else None
        lines.append(f"| {case} | {display(ratio)} | {display(iratio)} | {display(dratio)} | {display(median(rec,'tape_bytes'))} |")
    lines += ['', '## Free-mode floor gates','', '| Workload | Width | Free/tokio throughput | Instructions free/tokio | Record/off | p99 ratio | B/event | Durable/coop-1 |', '|---|---:|---:|---:|---:|---:|---:|---:|']
    for row in result['floors']:
        lines.append('| '+row['case']+' | '+str(row['width'])+' | '+' | '.join(display(row[k]) for k in ('free_vs_tokio','instructions_free_vs_tokio','record_vs_off','p99_record_vs_off','compressed_bytes_per_contended_event','durable_vs_cooperative_width1'))+' |')
    lines += ['', '## Per-row headroom gaps','', '| Row | Workload | Width | Metric | Candidate | Reference | Gap | Status |','|---|---|---:|---|---:|---:|---:|---|']
    for row in result['headroom']:
        lines.append(f"| {row['row']} | {row['case']} | {row['width']} | {row['metric']} | {display(row['free'])} | {display(row['tokio'])} | {display(row['gap'])} | {row['status']} |")
    lines += ['', '## Dependency-owned evidence','']+[f'- {key}: {value}.' for key,value in data['manifest']['dependency_owned'].items()]
    if data.get('errors'): lines += ['', '## Errors','']+[f'- {error}' for error in data['errors']]
    return '\n'.join(lines)+'\n'
