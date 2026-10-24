"""Seal evaluated worker tool identities before full-entry consumers. Setup only."""
import hashlib, json, os, pathlib, sys

def identity(path):
    path = pathlib.Path(path)
    h = hashlib.sha256()
    with path.open('rb') as source:
        for block in iter(lambda: source.read(1024*1024), b''):
            h.update(block)
    return dict(path=str(path), resolved=str(path.resolve()), bytes=path.stat().st_size, sha256=h.hexdigest())

files, environment = {}, {}
for key in ('HIRSEL_PRODUCTS','HIRSEL_RUNTIME_ARCHIVES','LLVM_SYS_231_PREFIX','HIRSEL_LINK_KIT','HIRSEL_GLIBC_FLOOR'):
    value = os.environ.get(key)
    environment[key] = value
    if not value: continue
    root = pathlib.Path(value)
    members = [root] if root.is_file() else sorted(root.rglob('*'))
    for path in members:
        if path.is_file(): files[str(path)] = identity(path)
for value in sys.argv[2:]:
    path = pathlib.Path(value)
    if path.is_file(): files[str(path)] = identity(path)
record = dict(environment=environment, files=files)
out=pathlib.Path(sys.argv[1]);out.write_text(json.dumps(record,sort_keys=True,indent=2)+'\n')
print('TOY_RUNTIME_SEAL',json.dumps(dict(path=str(out),sha256=identity(out)['sha256'],environment=environment,files=len(files)),sort_keys=True),flush=True)
# Emit full member hashes to the retained test log, without tool bytes or secrets.
for path, item in sorted(files.items()): print('TOY_TOOL',json.dumps(item,sort_keys=True),flush=True)
