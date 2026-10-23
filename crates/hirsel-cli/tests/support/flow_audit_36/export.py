"""Transport raw test evidence through retained test stdout, with optional exact interpreter bytes."""
import base64, hashlib, io, json, os, pathlib, sys, tarfile
root = pathlib.Path(sys.argv[1])
buffer = io.BytesIO()
with tarfile.open(fileobj=buffer, mode='w:gz') as archive:
    if os.environ.get('FLOW_AUDIT_36_EXPORT_PYTHON') == '1':
        archive.add(pathlib.Path(sys.executable).resolve(), arcname='tool-python3', recursive=False)
    for path in sorted(root.rglob('*')):
        relative = path.relative_to(root)
        if ('closure' in relative.parts and path.name != 'seal.json') or set(relative.parts) & {'runtime', 'blobs', '__pycache__'} or not path.is_file() or path.is_symlink():
            continue
        with path.open('rb') as source:
            if source.read(4) == b'\x7fELF' or path.suffix in {'.o', '.a', '.so'}:
                continue
        archive.add(path, arcname=str(relative), recursive=False)
data = buffer.getvalue()
print('FLOW36_ARCHIVE', json.dumps({'name': root.name, 'bytes': len(data), 'sha256': hashlib.sha256(data).hexdigest()}))
encoded = base64.b64encode(data).decode()
for offset in range(0, len(encoded), 16384):
    print('FLOW36_DATA', encoded[offset:offset+16384])
print('FLOW36_END', root.name, flush=True)
