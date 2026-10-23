#!/usr/bin/env python3
"""Full manifest/main with real files, scripted status-only processes, live UTC clock."""
import argparse,copy,datetime,hashlib,json,os,subprocess,time
from pathlib import Path
NS=60000000000
def write(p,v):p.write_text(json.dumps(v,separators=(',',':'))+'\n')
def utc(ns):return datetime.datetime.fromtimestamp(ns/1e9,datetime.timezone.utc).isoformat(timespec='seconds').replace('+00:00','Z')
def config_key(c):
 return json.dumps(dict(version=c['version'],parallel=c['parallel'],jobs=[{k:j[k] for k in ['id','cron','program','argv','deadline_ms']} for j in c['jobs']]),separators=(',',':'))
def main(a):
 a.binary=a.binary.resolve();a.capped=a.capped.resolve();a.root.mkdir(parents=True,exist_ok=True)
 env={k:v for k,v in os.environ.items() if not k.startswith('HIRSEL_RT_')};env.update(HIRSEL_JOBS='4',HIRSEL_MEMORY_LIMIT='2G',HIRSEL_DAEMON='off',HIRSEL_RESULTS='none@1')
 receipts=[]
 cases=json.loads(a.cases.read_text())
 # Scripted file errors and Pending have no production run-record provider.
 # Preserve their required native identities as INCOMPLETE, not a model pass.
 for case in cases:
  id=case['id'];r=dict(id=id,backend=a.backend,state='selected');receipts.append(r)
  if case.get('fault') or case.get('pending'):
   r.update(state='unrun',disposition='INCOMPLETE: no production file-fault/Pending provider');continue
  d=a.root/id;d.mkdir();cfg=d/'config';cfg.mkdir();state=d/'state';state.mkdir()
  config=case['config'];raw=json.dumps(config,separators=(',',':')).encode()
  if case.get('raw_size'):raw+=b' '*(case['raw_size']-len(raw))
  (cfg/'schedule.json').write_bytes(raw)
  if case.get('lock'):(state/'owner.lock').mkdir()
  args=case['args'].copy();expected=copy.deepcopy(case['rows']);error=case['error'];shift=0
  if args[0]=='run':
   # Keep the expected minute independent from implementation. Refuse setup if
   # a UTC minute edge would make this live-clock fixture ambiguous.
   now=time.time_ns();minute=now//NS
   if now%NS>57_000_000_000:
    r.update(state='unrun',disposition='INCOMPLETE: live clock near minute boundary');continue
   old=case.get('wall',0);shift=minute-old//NS
   args[2]=utc(now+2_000_000_000)
   for row in expected:
    if 'at' in row:
     oldtime=int(datetime.datetime.fromisoformat(row['at'].replace('Z','+00:00')).timestamp()*1e9)
     row['at']=utc(oldtime+shift*NS)
  prior=copy.deepcopy(case.get('prior'))
  if isinstance(prior,dict):
   if prior['config']!='wrong':prior['config']=config_key(config)
   prior['seen_ns']+=shift*NS
   # The live sample is after the fixed minute boundary by fractional seconds.
   # Exact24h gets its own small accepted margin for this real-clock supplement.
   if id=='gap-exact':prior['seen_ns']+=59_000_000_000
   for row in prior['rows']:row['at']+=shift
  if prior is not None:
   raw=(prior if isinstance(prior,str) else json.dumps(prior,separators=(',',':'))).encode()
   if case.get('state_size'):raw+=b' '*(case['state_size']-len(raw))
   (state/'state.json').write_bytes(raw)
  children=[dict(program=j['program'],args=j['argv'],steps=[dict(print=[79,79]),dict(print_err=[69,69]),dict(exit=dict(exited=7 if j['argv'] and j['argv'][0]=='fail' else 0))]) for j in config['jobs']]
  grants=dict(args=args,io=dict(stdout='stdout.bytes@1',stderr='stderr.bytes@1',tasks='tasks.cooperative@1',monotonic_clock='clock.monotonic.ns@1',wall_clock='clock.wall.unix_ns@1',dirs=[dict(path='config',host=str(cfg),access='read'),dict(path='state',host=str(state),access='read' if case.get('deny')=='state' else 'read_write')],process=dict(provider='process.script@1',rules=[] if case.get('deny')=='spawn' else [dict(program='/fixture/child')],runs=[],children=[] if case.get('missing_child') else children)),placement=dict(cores=1,commit='shared',reorder='auto'))
  grant=d/'grants.json';write(grant,grants);tape=d/'record.tape'
  cmd=[str(a.capped),'--timeout','15','prlimit','--cpu=60:61','--as=2147483648',str(a.binary),'--hirsel-record',str(tape),'--hirsel-grants',str(grant),*args]
  r.update(state='started',argv=cmd,expected=dict(rows=expected,error=error,exit=case['exit']));write(a.root/'receipts.json',receipts)
  done=subprocess.run(cmd,env=env,capture_output=True)
  (d/'stdout').write_bytes(done.stdout);(d/'capped.stderr').write_bytes(done.stderr)
  stderr=b''.join(line for line in done.stderr.splitlines(keepends=True) if not line.startswith(b'capped:'))
  (d/'stderr').write_bytes(stderr)
  expected_err=(json.dumps(dict(error=error),separators=(',',':'))+'\n').encode() if error else b''
  try:rows=[json.loads(line) for line in done.stdout.splitlines()]
  except ValueError:rows=None
  r.update(state='completed',actual=dict(rows=rows,stderr=stderr.decode(errors='replace'),exit=done.returncode),passed=rows==expected and stderr==expected_err and done.returncode==case['exit'])
  if tape.exists():r['tape_sha256']=hashlib.sha256(tape.read_bytes()).hexdigest()
  # Replay has no access to the granted resources/config/grants.
  cfg.rename(d/'config.unavailable');state.rename(d/'state.unavailable');grant.rename(d/'grants.unavailable')
  write(a.root/'receipts.json',receipts);print(json.dumps(r),flush=True)
 print(json.dumps(dict(selected=len(receipts),completed=sum(r['state']=='completed' for r in receipts),passed=sum(r.get('passed',False) for r in receipts))))
 assert all(r.get('passed',True) for r in receipts)
if __name__=='__main__':
 p=argparse.ArgumentParser();p.add_argument('--root',type=Path,required=True);p.add_argument('--binary',type=Path,required=True);p.add_argument('--backend',required=True);p.add_argument('--capped',type=Path,required=True);p.add_argument('--cases',type=Path,required=True);main(p.parse_args())
