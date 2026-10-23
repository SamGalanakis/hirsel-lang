"""Hash the concrete source/config/tool/provider union immediately before a consumer."""
import hashlib
import json
import os
from pathlib import Path
import sys

root=Path(sys.argv[1]);destination=Path(sys.argv[2]);argv=json.loads(Path(sys.argv[3]).read_text())
files={};directories={};seen=set();reused=[]
cache_file=root/'immutable-tool-digests.json'
cache=json.loads(cache_file.read_text()) if cache_file.exists() else {}
def file(path, immutable=False):
    p=Path(path)
    if p.is_file():
        state=p.stat()
        identity=[state.st_dev,state.st_ino,state.st_size,state.st_mtime_ns,state.st_ctime_ns]
        prior=cache.get(str(p))
        if immutable and prior and prior['identity']==identity:
            files[str(p)]=prior['sha256'];reused.append(str(p));return
        h=hashlib.sha256()
        with p.open('rb') as source:
            for block in iter(lambda:source.read(1024*1024),b''):h.update(block)
        files[str(p)]=h.hexdigest()
        if immutable:cache[str(p)]={'identity':identity,'sha256':h.hexdigest()}
def tree(path, immutable=False):
    p=Path(path)
    if p.is_file():file(p,immutable);return
    if not p.is_dir():return
    members=sorted(x for x in p.rglob('*') if x.is_file())
    directories[str(p)]=[str(x) for x in members]
    for x in members:file(x,immutable)
def package(path):
    p=Path(path).resolve()
    if p in seen:return
    seen.add(p);file(p);m=json.loads(p.read_text())
    for source in m['sources']:file(p.parent/source['path'])
    for dependency in m['dependencies']:package(p.parent/dependency['manifest'])
package(root/'hirsel.json')
for name in ['vectors.py','child.py','capped','seal.py']:file(root/name)
file(sys.executable)
for arg in argv:
    if arg.startswith('/'):
        p=Path(arg)
        if p.is_file():
            file(p)
            if p.suffix=='.json':
                try:config=json.loads(p.read_text())
                except (ValueError,UnicodeError):continue
                if isinstance(config,dict):
                    for mount in config.get('io',{}).get('dirs',[]):tree(mount['host'])
env={name:value for name,value in os.environ.items() if name.startswith(('HIRSEL_','LLVM_')) or name in ['PATH','LD_LIBRARY_PATH']}
for name in ['HIRSEL_IMAGE','HIRSEL_RUNTIME_ARCHIVES','HIRSEL_LINK_KIT','HIRSEL_GLIBC_FLOOR','LLVM_SYS_231_PREFIX']:
    if name in os.environ:tree(os.environ[name],immutable=True)
cache_file.write_text(json.dumps(cache))
destination.write_text(json.dumps(dict(immutable_digest_reuse=reused,cache_contract='prior full byte digest reused only with equal dev/ino/size/mtime_ns/ctime_ns; directory memberships re-evaluated',argv=argv,cwd=os.getcwd(),env=env,files=files,directories=directories,timing='before consumer'),indent=2))
