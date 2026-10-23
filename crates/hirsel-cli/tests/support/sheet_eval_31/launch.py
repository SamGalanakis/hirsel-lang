"""Keep capped's resource report separate from the application's exact streams."""
import os
from pathlib import Path
import subprocess
import sys
import tempfile
root=Path(__file__).resolve().parent
if len(sys.argv)>1 and sys.argv[1]=='--payload':
    with open(sys.argv[2],'wb') as out,open(sys.argv[3],'wb') as err:
        run=subprocess.run(sys.argv[4:],stdin=subprocess.DEVNULL,stdout=out,stderr=err)
    sys.exit(run.returncode)
dir=Path(tempfile.mkdtemp(prefix='launch-',dir=root))
argv=[(sys.executable or "/usr/bin/python3"),str(root/'capped'),'--timeout','60','prlimit','--cpu=60:61',(sys.executable or "/usr/bin/python3"),__file__,'--payload',str(dir/'stdout'),str(dir/'stderr'),*sys.argv[1:]]
with (dir/'scope').open('wb') as report:
    run=subprocess.run(argv,stdin=subprocess.DEVNULL,stdout=report,stderr=report,env=os.environ|{'HIRSEL_MEMORY_LIMIT':'2G','HIRSEL_JOBS':'4'})
for name,stream in [('stdout',sys.stdout.buffer),('stderr',sys.stderr.buffer)]:
    path=dir/name
    if path.exists():stream.write(path.read_bytes());stream.flush()
sys.exit(run.returncode)
