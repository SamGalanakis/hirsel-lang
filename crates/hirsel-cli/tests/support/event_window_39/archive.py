"""Lossless returned-log transport, adapted from merge_audit_37/archive.py.

Archives inputs, per-cell observations/receipts and original tape bytes. Large
compiler/runtime tools are represented by the before-consumer SHA inventory.
"""
import base64
import gzip
import hashlib
import io
import pathlib
import sys
import tarfile

root = pathlib.Path(sys.argv[1])
def emit(paths):
    stream = io.BytesIO()
    with tarfile.open(fileobj=stream, mode='w') as archive:
        for path in paths:
            archive.add(path, arcname=str(path.relative_to(root)), recursive=False)
    body = gzip.compress(stream.getvalue(), compresslevel=9, mtime=0)
    encoded = base64.b64encode(body)
    if len(encoded) > 4 * 1024 * 1024:
        raise SystemExit(f'EVENT_WINDOW_ARCHIVE_FAILURE encoded={len(encoded)} exceeds 4MiB; split cells')
    digest = hashlib.sha256(body).hexdigest()
    print(f'EVENT_WINDOW_39_ARCHIVE_BEGIN {digest} {len(body)} {len(encoded)}', flush=True)
    for start in range(0, len(encoded), 1024):
        print('EVENT_WINDOW_39_ARCHIVE_DATA ' + digest + ' ' + encoded[start:start + 1024].decode('ascii'), flush=True)
    print(f'EVENT_WINDOW_39_ARCHIVE_END {digest}', flush=True)

ordinary, objects = [], []
for path in sorted(root.rglob('*')):
    if not path.is_file() or path.is_symlink():
        continue
    relative = path.relative_to(root)
    if 'object-inputs' in relative.parts:
        objects.append(path)
        continue
    if any(part in {'tool-blobs', 'runtime', 'root', 'objects'} for part in relative.parts):
        continue
    if (path.suffix in {'.hir', '.json', '.jsonl', '.txt', '.tape', '.py', '.c', '.h', '.receipt', '.observation'}
            or path.name.endswith(('-stdout','-stderr'))
            or path.name in {'recording', 'binding', 'request', 'missing', 'extra', 'capped', 'stdout', 'stderr', 'cap-stdout', 'cap-stderr'}):
        ordinary.append(path)
emit(ordinary)
for path in objects:
    emit([path])
