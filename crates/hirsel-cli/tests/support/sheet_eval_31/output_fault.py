import os,sys,json,subprocess
from pathlib import Path
root=Path(sys.argv[1]);capped=sys.argv[2]
if len(sys.argv)>3 and sys.argv[3]=='--payload':
 with open('/dev/full','wb',buffering=0) as sink:
  done=subprocess.run(sys.argv[4:],stdin=subprocess.DEVNULL,stdout=sink)
 sys.exit(done.returncode)
for backend in ['llvm','cranelift']:
 d=root/f'output-fault-{backend}';d.mkdir();work=d/'work';work.mkdir();(work/'script').write_text('recalc\n')
 grants=d/'grants.json';grants.write_text(json.dumps({'args':['--jobs','4','script'],'io':{'stdout':'stdout.bytes@1','tasks':'tasks.cooperative@1','dirs':[dict(path='work',host=str(work),access='read_write')]},'placement':{'cores':1,'commit':'shared','reorder':'auto'}}))
 argv=[str(root/f'app-{backend}'),'--hirsel-record',str(d/'recording'),'--hirsel-grants',str(grants),'--jobs','4','script']
 (d/'argv.json').write_text(json.dumps(argv))
 env=os.environ|{'HIRSEL_MEMORY_LIMIT':'2G','HIRSEL_JOBS':'4'}
 with (d/'stdout').open('wb') as out,(d/'stderr').open('wb') as err:
  done=subprocess.run([capped,'--timeout','60','prlimit','--cpu=60:61',sys.executable,__file__,str(root),capped,'--payload',*argv],stdout=out,stderr=err,env=env)
 assert done.returncode==3,(backend,done.returncode,(d/'stderr').read_text());assert (d/'stdout').read_bytes()==b''
 (d/'exit').write_text('3');(d/'PASS').write_text('real stdout ENOSPC, explicit ExitCode3\n');work.rename(d/'original-work-unavailable')
 print('OUTPUT_FAULT_PASS',backend,flush=True)
