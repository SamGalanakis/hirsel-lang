"""Lane-local durable worker transport, authorized by coordinator 656880.
One archive per cell, at most 4 MiB base64. Never discard a file to fit a cell.
Tool bytes outside the archive bind by SHA to the host's retained Buck inputs.
"""
import base64,hashlib,io,json,os,shutil,subprocess,sys,tarfile
from pathlib import Path
root=Path(sys.argv[1])
external=[]
# Common immutable compiler/tool bytes already retained on the submitting host
# bind by digest. Program images, objects, raw tapes, inputs and sources stay in
# the archive. The extractor refuses credit unless every external byte resolves.
for p in sorted(root.rglob('*')):
    if p.is_file() and (p.relative_to(root).parts[0]=='closure' or '/runtime/runners/' in str(p)):
        external.append(dict(path=str(p.relative_to(root)),sha256=hashlib.sha256(p.read_bytes()).hexdigest(),bytes=p.stat().st_size))
if (root/'consumer-inputs.json').exists():
    for row in json.loads((root/'consumer-inputs.json').read_text()):
        if row['name']!='cell':
            external.append(dict(path='consumer/'+row['name']+'/'+row['path'].lstrip('/'),sha256=row['sha256'],bytes=row['bytes']))
(root/'external-inputs.json').write_text(json.dumps(external,indent=2)+'\n')
excluded={r['path'] for r in external}
buf=io.BytesIO()
with tarfile.open(fileobj=buf,mode='w') as tar:
    for p in sorted(root.rglob('*')):
        if p.is_file() and not p.is_symlink() and str(p.relative_to(root)) not in excluded:
            tar.add(p,arcname=str(p.relative_to(root)),recursive=False)
raw=buf.getvalue()
if shutil.which('zstd'):
    blob=subprocess.run(['zstd','-q','-3','-c'],input=raw,stdout=subprocess.PIPE,check=True).stdout
    codec='zstd'
else:
    import gzip
    blob=gzip.compress(raw,compresslevel=3,mtime=0);codec='gzip'
encoded=base64.b64encode(blob)
assert len(encoded)<=4*1024*1024, f'INCOMPLETE archive {len(encoded)} exceeds 4MiB; preserve this missing cell'
print('DT35_ARCHIVE_BEGIN',hashlib.sha256(blob).hexdigest(),len(blob),codec,flush=True)
print(encoded.decode(),flush=True)
print('DT35_ARCHIVE_END',flush=True)
