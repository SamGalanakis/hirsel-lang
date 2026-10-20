"""Ten interleaved, seeded profiler-on/off samples; timings are within-trace models."""
import hashlib, importlib.util, json, os, random, selectors, socket, statistics, subprocess, sys, time
from pathlib import Path
ROOT=Path(__file__).resolve().parents[2]
OUT=Path(sys.argv[1]).resolve()
spec=importlib.util.spec_from_file_location('mc',ROOT/'tools/mc-profile.py')
mc=importlib.util.module_from_spec(spec);spec.loader.exec_module(mc)
ENV=json.loads((OUT/'environment.json').read_text())
ENV.pop('HIRSEL_BENCH_MEASUREMENT',None)
NAMES=['tasks-app','CONC-2-task-all-ok-run','MEM-11-tour-11-shared-config-run','CONC-2-task-select-fast-slow-run','CONC-3-check-canceled-run','CLO-3-child-mutex-capture-run','resource-drops']
SEED=20261020

def sha(p):return hashlib.sha256(p.read_bytes()).hexdigest()
def cpu():
 for line in Path('/proc/stat').read_text().splitlines():
  if line.startswith('cpu15 '):
   values=list(map(int,line.split()[1:9]));return sum(values),values[3]+values[4]
def busy(a,b):
 total=b[0]-a[0]
 return 1-(b[1]-a[1])/total if total else None

def command(binary,env,directory):
 return [str(ROOT/'tools/capped'),'--timeout','120','perf','stat','-x,','-e','cycles,instructions','-o',str(directory/'perf.csv'),'--','taskset','-c','31',str(binary)]

def execute(name,enabled,round):
 binary=OUT/name/'program';d=OUT/'samples'/str(round)/f'{name}-{enabled}';d.mkdir(parents=True,exist_ok=True)
 env=dict(ENV,HIRSEL_RT_MEASUREMENT=str(d/'measurement'))
 if enabled:env['HIRSEL_RT_MC_PROFILE']='1'
 argv=command(binary,env,d)
 before=cpu()
 if name!='tasks-app':
  fixture=OUT/'input-root';fixture.mkdir(exist_ok=True)
  env['HIRSEL_RT_ROOT']=str(fixture)
  env['HIRSEL_RT_TAPE']=str(d/'tape')
  r=subprocess.run(argv,env=env,cwd=ROOT,capture_output=True)
  (d/'stdout').write_bytes(r.stdout);(d/'stderr').write_bytes(r.stderr)
  if r.returncode:raise RuntimeError((name,r.returncode,r.stderr.decode()))
 else:
  with socket.socket() as sock:
   sock.bind(('127.0.0.1',0));port=sock.getsockname()[1]
  data=d/'data';data.mkdir()
  grants=json.loads((ROOT/'examples/tasks/grants.json').read_text())
  grants['io'].pop('wall_clock',None)
  grants['io']['dirs'][0]['host']=str(data)
  grants['io']['dirs'][1]['host']=str(ROOT/'examples/tasks/static')
  grants['io']['net']['rules'][0]['service']=str(port)
  grants['args']=[]
  g=d/'grants.json';g.write_text(json.dumps(grants))
  argv+=['--hirsel-grants',str(g),'--port',str(port)]
  with (d/'stderr').open('wb') as err:
   server=subprocess.Popen(argv,env=env,cwd=ROOT,stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=err)
   line=b''
   try:
    with selectors.DefaultSelector() as sel:
     sel.register(server.stdout,selectors.EVENT_READ)
     if not sel.select(30):raise RuntimeError('server readiness timeout')
     line=server.stdout.readline()
    if b'listening on' not in line:raise RuntimeError(('server readiness',line,(d/'stderr').read_text()))
    load=subprocess.run([str(ROOT/'tools/capped'),'--timeout','60','taskset','-c','7',sys.executable,str(Path(__file__).with_name('load.py')),str(port)],capture_output=True)
    (d/'client.stdout').write_bytes(load.stdout);(d/'client.stderr').write_bytes(load.stderr)
    if load.returncode:raise RuntimeError(('client',load.stderr.decode()))
   finally:
    stdout,_=server.communicate(b'quit\n',timeout=45)
    (d/'stdout').write_bytes(line+stdout)
   if server.returncode:raise RuntimeError(('server',server.returncode,(d/'stderr').read_text()))
 after=cpu()
 counters={}
 for line in (d/'perf.csv').read_text().splitlines():
  cols=line.split(',')
  if len(cols)>2 and cols[2] in ('cycles','instructions'):
   counters[cols[2]]=int(cols[0])
 item=dict(program=name,round=round,enabled=enabled,sibling_busy=busy(before,after),counters=counters,binary_sha256=sha(binary))
 if enabled:
  rows=mc.read_log(d/'measurement')
  item['model']=mc.report(rows,200)
  item['segment_ns_median']=statistics.median(r['ns'] for r in rows)
  item['segment_ns_max']=max(r['ns'] for r in rows)
  item['trace_sha256']=sha(d/'measurement')
  item['sensitivity']={str(cost):mc.report(rows,cost)['results'][2]['speedup'] for cost in (0,100,500,1000)}
 return item

def main():
 rng=random.Random(SEED);samples=[]
 selected=NAMES if len(sys.argv)<3 else sys.argv[2:]
 rounds=10 if os.environ.get('MC_SMOKE') is None else 1
 for round in range(rounds):
  jobs=[(n,b) for n in selected for b in (False,True)];rng.shuffle(jobs)
  for name,enabled in jobs:
   item=execute(name,enabled,round);samples.append(item)
   print(round,name,enabled,item.get('model',{}).get('tasks'),flush=True)
   (OUT/'samples.json').write_text(json.dumps(dict(seed=SEED,samples=samples),indent=2))
  for name in selected:
   if name=='tasks-app':continue
   a=OUT/'samples'/str(round)/f'{name}-False';b=OUT/'samples'/str(round)/f'{name}-True'
   assert (a/'stdout').read_bytes()==(b/'stdout').read_bytes()
   assert (a/'tape').read_bytes()==(b/'tape').read_bytes()
if __name__=='__main__':main()
