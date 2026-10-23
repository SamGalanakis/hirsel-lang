"""Lane-only returned-log transport, authorized by agentlang-24 (656743)."""
import base64
import gzip
import hashlib
import io
import pathlib
import sys
import tarfile

root = pathlib.Path(sys.argv[1])
stream = io.BytesIO()
with tarfile.open(fileobj=stream, mode='w') as archive:
    for path in sorted(root.rglob('*')):
        if not path.is_file() or path.is_symlink():
            continue
        relative = path.relative_to(root)
        if any(part in {'tool-blobs', 'runtime', 'root', 'objects'} for part in relative.parts):
            continue
        if (path.suffix in {'.hir', '.json', '.jsonl', '.txt', '.tape', '.py'}
                or path.name in {'recording', 'capped', 'stdout', 'stderr', 'cap-stdout', 'cap-stderr'}):
            archive.add(path, arcname=str(relative), recursive=False)
body = gzip.compress(stream.getvalue(), compresslevel=9, mtime=0)
encoded = base64.b64encode(body)
if len(encoded) > 4 * 1024 * 1024:
    raise SystemExit(f'SPECTRAL_ARCHIVE_FAILURE encoded={len(encoded)} exceeds 4MiB; split cells')
digest = hashlib.sha256(body).hexdigest()
print(f'SPECTRAL35_ARCHIVE_BEGIN {digest} {len(body)} {len(encoded)}', flush=True)
for start in range(0, len(encoded), 1024):
    print(encoded[start:start + 1024].decode('ascii'), flush=True)
print(f'SPECTRAL35_ARCHIVE_END {digest}', flush=True)
