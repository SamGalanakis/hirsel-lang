"""Paired actual-tool and immutable-fixture inventories, before/after consumers."""
import hashlib
import json
import os
import pathlib
import sys

root = pathlib.Path(sys.argv[2])
files, members, symlinks, probes, cycles = {}, {}, {}, {}, []
visited = set()


def visit(path, ancestors=frozenset(), required=False):
    path = pathlib.Path(path)
    probes[str(path)] = dict(exists=path.exists(), file=path.is_file(), directory=path.is_dir(), symlink=path.is_symlink())
    if path.is_symlink():
        symlinks[str(path)] = os.readlink(path)
    if not path.exists():
        if required:
            raise FileNotFoundError(f'required consumed input: {path}')
        return
    resolved = path.resolve(strict=True)
    stat = resolved.stat()
    identity = stat.st_dev, stat.st_ino
    if path.is_dir():
        children = sorted(path.iterdir(), key=lambda p: p.name)
        members[str(path)] = [dict(name=p.name, file=p.is_file(), directory=p.is_dir(),
                                  link=os.readlink(p) if p.is_symlink() else None) for p in children]
        if identity in ancestors:
            cycles.append(str(path))
            return
        if identity in visited:
            return
        visited.add(identity)
        for child in children:
            visit(child, ancestors | {identity})
    elif path.is_file():
        with path.open('rb') as source:
            files[str(path)] = dict(sha256=hashlib.file_digest(source, 'sha256').hexdigest(),
                                    bytes=stat.st_size, resolved=str(resolved))


for name in ['HIRSEL_IMAGE', 'HIRSEL_RUNTIME_ARCHIVES', 'LLVM_SYS_231_PREFIX',
             'HIRSEL_LINK_KIT', 'HIRSEL_GLIBC_FLOOR']:
    if name in os.environ:
        visit(os.environ[name], required=True)
for value in sys.argv[3:]:
    visit(value, required=True)
for name in ['main.hir', 'model.hir', 'input.hir', 'window.hir', 'checkpoint.hir',
             'report.hir', 'hirsel.json', 'oracle.py', 'archive.py', 'seal.py',
             'capped', 'contracts.hir', 'oracle-cases.json', 'selected.json', 'limits.json']:
    path = root / name
    visit(path)
for name in ['/usr/bin/python3', '/usr/bin/env', '/usr/bin/prlimit', '/usr/bin/systemd-run',
             '/usr/lib/x86_64-linux-gnu/libc.so.6', '/lib64/ld-linux-x86-64.so.2']:
    visit(name)
config = {k: v for k, v in os.environ.items()
          if k.startswith(('HIRSEL_', 'LLVM_', 'EVENT_WINDOW_'))}
current = dict(files=files, members=members, symlinks=symlinks, probes=probes, cycles=cycles, config=config)
if sys.argv[1] == 'before':
    phases = len(list(root.glob('actual-before-*.json')))
    (root / f'actual-before-{phases}.json').write_text(json.dumps(current, sort_keys=True, indent=2)+'\n')
    (root / 'actual-inputs.json').write_text(json.dumps(current, sort_keys=True, indent=2)+'\n')
    print(f'EVENT_WINDOW_39_ACTUAL_BEFORE phase={phases} files={len(files)} directories={len(members)}', flush=True)
else:
    expected = json.loads((root / 'actual-inputs.json').read_text())
    differences = [k for k in current if current[k] != expected[k]]
    (root / 'actual-after.json').write_text(json.dumps(current, sort_keys=True, indent=2)+'\n')
    (root / 'actual-paired.json').write_text(json.dumps(dict(equal=not differences, differences=differences))+'\n')
    print(f'EVENT_WINDOW_39_ACTUAL_PAIRED equal={not differences} differences={differences}', flush=True)
    sys.exit(bool(differences))
