"""Lane-local durable worker transport, authorized by coordinator 656880.
One archive per cell, at most 4 MiB base64. Omitted failure capsules never pass.
Tool bytes outside the archive bind by SHA to the host's retained Buck inputs.
"""
import base64,hashlib,io,json,os,shutil,subprocess,sys,tarfile
from pathlib import Path
root=Path(sys.argv[1])
external=[]
known=set(json.loads(os.environ.get("DECISION_TREE_35_KNOWN_ARTIFACTS", "[]")))
# Common immutable compiler/tool bytes already retained on the submitting host
# bind by digest. Known retained program artifacts may also bind by SHA; all
# other program bytes stay in the archive. Every external byte must resolve.
for p in sorted(root.rglob('*')):
    if p.is_file() and (p.relative_to(root).parts[0]=='closure' or p==root/'qualified.image' or (p.suffix in ['.o','.so'] or p.name=='hirsel-runner') and hashlib.sha256(p.read_bytes()).hexdigest() in known):
        external.append(dict(path=str(p.relative_to(root)),sha256=hashlib.sha256(p.read_bytes()).hexdigest(),bytes=p.stat().st_size))
if (root/'consumer-inputs.json').exists():
    for row in json.loads((root/'consumer-inputs.json').read_text()):
        if row['name']!='cell':
            external.append(dict(path='consumer/'+row['name']+'/'+row['path'].lstrip('/'),sha256=row['sha256'],bytes=row['bytes']))
(root/'external-inputs.json').write_text(json.dumps(external,indent=2)+'\n')
excluded={r['path'] for r in external}
def pack(omitted):
    buf=io.BytesIO()
    with tarfile.open(fileobj=buf,mode='w') as tar:
        for p in sorted(root.rglob('*')):
            key=str(p.relative_to(root))
            if p.is_file() and not p.is_symlink() and key not in excluded and key not in omitted:
                tar.add(p,arcname=key,recursive=False)
    raw=buf.getvalue()
    if shutil.which('zstd'):
        blob=subprocess.run(['zstd','-q','-3','-c'],input=raw,stdout=subprocess.PIPE,check=True).stdout
        codec='zstd'
    else:
        # stdlib liblzma compression keeps native executable bytes inside the
        # same fixed transport cap when a zstd executable is unavailable.
        import lzma
        blob=lzma.compress(raw,preset=6);codec='lzma'
    if codec=='zstd' and len(base64.b64encode(blob))>4*1024*1024:
        import lzma
        blob=lzma.compress(raw,preset=6);codec='lzma'
    return blob,codec

omitted={}
blob,codec=pack(omitted)
if len(base64.b64encode(blob))>4*1024*1024:
    # Always retain raw tapes/receipts/source on failure. Missing executable
    # bytes are an explicit incomplete closure, never an accepted smaller cell.
    candidates=sorted((p for p in root.rglob('*') if p.is_file() and
        (p.suffix in ['.o','.so'] or p.name.startswith('ordered-') or p.name=='hirsel-runner') and str(p.relative_to(root)) not in excluded),key=lambda p:p.stat().st_size,reverse=True)
    for p in candidates:
        key=str(p.relative_to(root));omitted[key]=dict(sha256=hashlib.sha256(p.read_bytes()).hexdigest(),bytes=p.stat().st_size)
        (root/'omitted-for-cap.json').write_text(json.dumps(dict(full_encoded_bytes=len(base64.b64encode(blob)),cap=4*1024*1024,omitted=omitted),indent=2)+'\n')
        blob,codec=pack(omitted)
        if len(base64.b64encode(blob))<=4*1024*1024:break
encoded=base64.b64encode(blob)
assert len(encoded)<=4*1024*1024, f'INCOMPLETE archive {len(encoded)} exceeds 4MiB'
print('DT35_ARCHIVE_BEGIN',hashlib.sha256(blob).hexdigest(),len(blob),codec,flush=True)
print(encoded.decode(),flush=True)
print('DT35_ARCHIVE_END',flush=True)
if omitted:
    print('INCOMPLETE: program artifact bytes omitted from bounded failure capsule',file=sys.stderr)
    sys.exit(2)
