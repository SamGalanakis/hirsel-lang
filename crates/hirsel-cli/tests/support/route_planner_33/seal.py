"""Retain concrete source/config/generated/tool inputs before a consumer."""
import hashlib, json, os, pathlib, subprocess, sys
root=pathlib.Path(sys.argv[1]).resolve(); out=pathlib.Path(sys.argv[2]); out.mkdir(parents=True,exist_ok=True)
files={}; dirs={}
def digest(p):
 h=hashlib.sha256()
 with p.open('rb') as f:
  for b in iter(lambda:f.read(1024*1024),b''):h.update(b)
 return h.hexdigest()
def file(p,archive=False):
 p=pathlib.Path(p)
 if not p.is_file():return
 files[str(p)]=dict(resolved=str(p.resolve()),bytes=p.stat().st_size,sha256=digest(p))
 if archive:
  target=out/'source'/p.relative_to(root);target.parent.mkdir(parents=True,exist_ok=True);target.write_bytes(p.read_bytes())
def tree(p):
 p=pathlib.Path(p)
 if p.is_file():file(p);return
 if not p.is_dir():return
 members=sorted(x for x in p.rglob('*') if x.is_file());dirs[str(p)]=[str(x) for x in members]
 for x in members:file(x)
try:
 tracked=subprocess.check_output(['git','ls-files','-z'],cwd=root,stderr=subprocess.DEVNULL).split(b'\0')
 source_scope='tracked checkout plus owned source'
 head=subprocess.check_output(['git','rev-parse','HEAD'],cwd=root,text=True).strip()
except (OSError,subprocess.CalledProcessError):
 # A remote test has only declared action inputs, without a .git directory.
 tracked=[os.fsencode(str(p.relative_to(root))) for p in root.rglob('*') if p.is_file() and not set(p.relative_to(root).parts)&{'buck-out','target','.git','.buck2'}]
 source_scope='declared sandbox inputs; Git inventory unavailable'
 head=None
for name in tracked:
 if name:file(root/os.fsdecode(name),True)
for prefix in ['examples/routeplan','crates/hirsel-cli/tests/support/route_planner_33']:
 for p in (root/prefix).rglob('*'):
  if p.is_file():file(p,True)
file(root/'crates/hirsel-cli/tests/cli/route_planner_33.rs',True)
for name in ['BUCK','third-party/BUCK','third-party/Cargo.toml','third-party/Cargo.lock','third-party/lib.rs','tools/buck2/exec_sizes.bzl','tools/buck2/test_sizes.bzl','tools/buck2/workspace-metadata.json','tools/native-toolchain/pins.bzl','.buck2/sync-stamp.json','.buckconfig','.buckconfig.local']:
 file(root/name,True)
env={k:v for k,v in os.environ.items() if k.startswith(('HIRSEL_','LLVM_','ROUTE_PLANNER_')) or k in ['PATH','LD_LIBRARY_PATH']}
for key in ['HIRSEL_IMAGE','HIRSEL_RUNTIME_ARCHIVES','HIRSEL_LINK_KIT','HIRSEL_GLIBC_FLOOR','LLVM_SYS_231_PREFIX']:
 if env.get(key):tree(env[key])
for path in sys.argv[3:]:file(path)
file(sys.executable)
(out/'seal.json').write_text(json.dumps(dict(cwd=str(root),head=head,source_scope=source_scope,environment=env,files=files,directories=dirs,timing='before consumer; input stamps include evaluated reads/probes/globs and outputs'),sort_keys=True,indent=2)+'\n')
print('ROUTE_SEAL',out,len(files),flush=True)
