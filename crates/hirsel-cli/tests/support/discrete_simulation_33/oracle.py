"""Independent integer-tick oracle; no heap, callbacks or Hirsel implementation.
SimPy MIT resource vectors: tests/test_resources.py:17-43 at f43816490.
"""
import copy
import json
from pathlib import Path

ABSENT = 1000001

def tick_model(config):
    jobs = [dict(state='future', visit=0, wait=0, finish=ABSENT, due=ABSENT, order=0) for _ in config['jobs']]
    queues = [[] for _ in config['stations']]
    busy = [0] * len(queues)
    area = [0] * len(queues)
    qarea = [0] * len(queues)
    ordinal = 0
    def start(j, now):
        nonlocal ordinal
        job = jobs[j]
        step = config['jobs'][j]['route'][job['visit']]
        s = step['station']
        job['state'] = 'service'
        job['due'] = now + step['duration']
        job['order'] = ordinal
        ordinal += 1
        busy[s] += 1
    def request(j, now):
        s = config['jobs'][j]['route'][jobs[j]['visit']]['station']
        if busy[s] < config['stations'][s]['capacity']:
            start(j, now)
        else:
            jobs[j]['state'] = 'waiting'
            queues[s].append(j)
    def snapshot(now):
        lines = [f'time {now}']
        for j, job in enumerate(jobs):
            lines.append(f"job {j} {job['state']} {job['visit']} {job['wait']} {job['finish']}")
        for s in range(len(queues)):
            lines.append(f'station {s} {busy[s]} {len(queues[s])} {area[s]} {qarea[s]}')
        return '\n'.join(lines) + '\nend\n'
    horizons = config['horizons']
    checkpoints = []
    last_event = 0
    now = 0
    while True:
        if now in horizons:
            checkpoints.append(snapshot(now))
            if now == horizons[-1]:
                break
        for j, spec in enumerate(config['jobs']):
            job = jobs[j]
            if spec['cancel'] == now and job['state'] in ('future', 'waiting'):
                if job['state'] == 'waiting':
                    queues[spec['route'][job['visit']]['station']].remove(j)
                job.update(state='canceled', finish=now)
                last_event = now
        releases = sorted((job['order'], j) for j, job in enumerate(jobs) if job['state'] == 'service' and job['due'] == now)
        for _, j in releases:
            job = jobs[j]
            s = config['jobs'][j]['route'][job['visit']]['station']
            busy[s] -= 1
            job['visit'] += 1
            if queues[s]:
                start(queues[s].pop(0), now)
            if job['visit'] == len(config['jobs'][j]['route']):
                job.update(state='completed', finish=now)
            else:
                request(j, now)
            last_event = now
        for j, spec in enumerate(config['jobs']):
            if spec['arrival'] == now:
                if jobs[j]['state'] == 'future':
                    request(j, now)
                last_event = now
            if spec['cancel'] == now:
                last_event = now
        pending = any(job['state'] in ('future', 'waiting', 'service') for job in jobs)
        pending |= any(spec['cancel'] != ABSENT and spec['cancel'] > now for spec in config['jobs'])
        pending |= any(spec['arrival'] > now for spec in config['jobs'])
        if not horizons and not pending:
            checkpoints.append(snapshot(last_event))
            break
        for s in range(len(queues)):
            area[s] += busy[s]
            qarea[s] += len(queues[s])
        for job in jobs:
            if job['state'] == 'waiting':
                job['wait'] += 1
        now += 1
        assert now <= 1000000
    return ''.join('replica ' + str(r) + '\n' + ''.join(checkpoints) for r in range(config['replicas']))

def scenario(capacity=1, durations=(1,1), horizons=(), replicas=1):
    return dict(version=1, replicas=replicas, stations=[dict(capacity=capacity)], jobs=[dict(arrival=0,cancel=ABSENT,route=[dict(station=0,duration=d)]) for d in durations], horizons=list(horizons))

def vectors():
    cases=[]
    def add(name, config, error=None, **extra):
        cases.append(dict(id=name, input=json.dumps(config,separators=(',',':')), jobs=extra.pop('jobs',1), stdout='' if error else tick_model(config), stderr=f'queuesim: {error}\n' if error else '', exit=2 if error else 0, **extra))
    add('empty', scenario(durations=()))
    add('simpy-fifo', scenario())
    add('capacity-two', scenario(2,(2,1,1)))
    c=scenario(durations=(2,1,1)); c['jobs'][2]['arrival']=2; add('simultaneous',c)
    c=scenario(durations=(3,1,1)); c['jobs'][1]['cancel']=1; add('waiting-cancel',c)
    c=scenario(durations=(1,)); c['jobs'][0].update(arrival=3,cancel=1); add('future-cancel',c)
    c=scenario(durations=(3,)); c['jobs'][0]['cancel']=1; add('service-cancel-ignored',c)
    c=scenario(durations=(1,1), horizons=(1,2,3)); add('resume',c)
    c=scenario(durations=(1,1)); c['stations'].append(dict(capacity=1)); c['jobs'][0]['route'].append(dict(station=1,duration=2)); add('route',c)
    for width in (1,2,4): add(f'batch-{width}',scenario(replicas=4),jobs=width)
    add('duration-zero',scenario(durations=(0,)), 'duration')
    add('denial',scenario(), 'input_grant',deny=True)
    add('pending',scenario(), 'canceled',pending=True)
    for field,cap in [('stations',16),('jobs',64),('replicas',4)]:
        c=scenario(durations=())
        if field=='stations': c[field]=[dict(capacity=1)]*(cap+1)
        elif field=='jobs': c[field]=scenario(durations=(1,)*(cap+1))['jobs']
        else: c[field]=cap+1
        add(f'over-{field}',c,field)
    c=scenario(); c['jobs'][0]['arrival']=ABSENT; add('over-tick',c,'tick')
    add('over-workers',scenario(),'workers',jobs=5)
    c=scenario(durations=(1,)); c['jobs'][0]['route']*=512; add('never-admitted-events',c,'events')
    c=scenario(durations=(1,)); c['jobs'][0]['route']*=511; add('cap-events',c)
    add('cap-jobs',scenario(durations=(1,)*64))
    c=scenario(durations=()); c['stations']*=16; add('cap-stations',c)
    c=scenario(durations=()); c['horizons']=[1000000]; add('cap-tick',c)
    c=scenario(durations=(1,)); c['jobs'][0]['arrival']=1000000; add('release-over-tick',c,'tick')
    c=scenario(durations=(18446744073709551615,)); add('over-duration',c,'duration')
    add('input-cap',scenario(durations=()),pad=131072)
    add('input-over',scenario(durations=()),'input_bytes',pad=131073)
    c=scenario(durations=(2,1,1)); c['jobs'][1]['cancel']=2; add('cancel-at-release',c)
    c=scenario(durations=(1,)); c['jobs'][0]['cancel']=0; add('cancel-at-arrival',c)
    add('horizon-zero',scenario(durations=(1,),horizons=(0,1,2)))
    c=scenario(durations=()); c['horizons']=[1,1]; add('duplicate-horizon',c,'horizon')
    c=scenario(durations=()); c['horizons']=list(range(17)); add('over-horizons',c,'horizons')
    c=scenario(); c['stations'][0]['capacity']=0; add('capacity-zero',c,'capacity')
    c=scenario(); c['jobs'][0]['route'][0]['station']=1; add('unknown-station',c,'station')
    c=scenario(); c['jobs'][0]['route']=[]; add('empty-route',c,'route')
    c=scenario(); c['jobs'][0]['cancel']=1000002; add('over-cancel-tick',c,'tick')
    c=scenario(durations=(1,)*28,horizons=range(16),replicas=4); c['stations']*=16
    for j in c['jobs']: j['arrival']=999999
    add('cap-output-exact',c)
    assert len(cases[-1]['stdout'].encode())==65536
    c['horizons'][-1]=100; add('over-output',c,'output_bytes')
    add('signal-denied',scenario(),'signal_wait',pending=True,signal_denied=True)
    return cases

if __name__=='__main__':
    path=Path(__file__).with_name('cases.json')
    import sys
    encoded=json.dumps(vectors(),indent=2)+'\n'
    if '--check' in sys.argv:
        assert path.read_text()==encoded, 'frozen oracle outputs changed'
    else:
        path.write_text(encoded)
