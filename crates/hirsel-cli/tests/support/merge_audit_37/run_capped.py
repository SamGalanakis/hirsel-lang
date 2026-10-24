"""Toolchain wrapper: retain cap diagnostics separately from application streams."""
import os, pathlib, subprocess, sys, tempfile
if sys.argv[1]=='child':
 target=pathlib.Path(sys.argv[2])
 with (target/'stdout').open('wb') as out,(target/'stderr').open('wb') as err:
  p=subprocess.run(sys.argv[3:],stdout=out,stderr=err)
 if p.returncode<0:
  import signal
  signal.signal(-p.returncode,signal.SIG_DFL);os.kill(os.getpid(),-p.returncode)
 sys.exit(p.returncode)
base=pathlib.Path(sys.argv[1]);target=pathlib.Path(tempfile.mkdtemp(prefix='launch-',dir=base))
env=os.environ|{'HIRSEL_JOBS':os.environ.get('HIRSEL_JOBS','1'),'HIRSEL_MEMORY_LIMIT':'2G','PATH':os.defpath}
cmd=['python3',str(base/'capped'),'--timeout','60','prlimit','--cpu=60:61','--data=2147483648:2147483648','python3',__file__,'child',str(target),*sys.argv[2:]]
(target/'command.json').write_text(__import__('json').dumps(cmd))
p=subprocess.run(cmd,env=env,stdout=subprocess.PIPE,stderr=subprocess.PIPE)
(target/'cap-stdout').write_bytes(p.stdout);(target/'cap-stderr').write_bytes(p.stderr)
for name,stream in [('stdout',sys.stdout.buffer),('stderr',sys.stderr.buffer)]:
 path=target/name
 if path.exists():stream.write(path.read_bytes());stream.flush()
if p.returncode!=0 and not (target/'stderr').exists():sys.stderr.buffer.write(p.stderr)
sys.exit(p.returncode)
