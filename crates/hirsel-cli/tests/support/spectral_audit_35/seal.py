"""Retain live source, evaluated generation inputs and concrete tool bytes."""
import hashlib
import json
import os
import pathlib
import shutil
import subprocess
import sys

root = pathlib.Path(sys.argv[1]).resolve()
out = pathlib.Path(sys.argv[2])
out.mkdir(parents=True, exist_ok=True)
blobs = pathlib.Path(os.environ.get('SPECTRAL_AUDIT_35_EVIDENCE', str(out.parent))) / 'tool-blobs'
blobs.mkdir(parents=True, exist_ok=True)
files, directories, cycles = {}, {}, []
transport = os.environ.get('SPECTRAL_AUDIT_35_TRANSPORT') == 'stdout'

def file(path, source=False):
    path = pathlib.Path(path)
    if not path.is_file():
        return
    h = hashlib.sha256()
    with path.open('rb') as stream:
        for data in iter(lambda: stream.read(1024 * 1024), b''):
            h.update(data)
    digest = h.hexdigest()
    target = blobs / digest
    if not transport and not target.exists():
        # Copy instead of hardlinking a mutable original into the archive.
        shutil.copyfile(path.resolve(), target)
    files[str(path)] = dict(resolved=str(path.resolve()), bytes=path.stat().st_size,
                            sha256=digest, retained_bytes=str(target))
    if source and path.is_relative_to(root):
        target = out / 'source' / path.relative_to(root)
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(path, target)

def members(path, ancestors=frozenset()):
    """Follow symlink directories while refusing only ancestor inode cycles."""
    path = pathlib.Path(path)
    if path.is_file():
        yield path
    elif path.is_dir():
        stat = path.stat()
        identity = (stat.st_dev, stat.st_ino)
        if identity in ancestors:
            cycles.append(str(path))
            return
        children = sorted(path.iterdir())
        directories[str(path)] = [str(p) for p in children]
        for child in children:
            yield from members(child, ancestors | {identity})

def tree(path, source=False):
    for member in members(path):
        file(member, source)

try:
    tracked = subprocess.check_output(['git', 'ls-files', '-z'], cwd=root,
                                      stderr=subprocess.DEVNULL).split(b'\0')
    head = subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=root, text=True).strip()
    scope = 'tracked checkout plus owned source'
    for name in tracked:
        if name:
            file(root / os.fsdecode(name), True)
except (OSError, subprocess.CalledProcessError):
    head = None
    scope = 'actual sandbox std/app sources and tool digests; complete tracked checkout and matching tool bytes retained by invoking-host seal'
    for directory in sorted(root.iterdir()):
        if directory.name in {'stdlib'}:
            tree(directory, True)

for name in ['examples/spectralaudit', 'crates/hirsel-cli/tests/support/spectral_audit_35']:
    tree(root / name, True)
file(root / 'crates/hirsel-cli/tests/cli/spectral_audit_35.rs', True)
for name in ['BUCK', 'third-party/BUCK', 'third-party/Cargo.toml', 'third-party/Cargo.lock',
             'third-party/lib.rs', 'tools/buck2/exec_sizes.bzl', 'tools/buck2/test_sizes.bzl',
             'tools/buck2/workspace-metadata.json', 'tools/native-toolchain/pins.bzl',
             '.buck2/sync-stamp.json', '.buckconfig', '.buckconfig.local']:
    file(root / name, True)

# The stamp is an inventory to re-evaluate, not proof of today's membership.
evaluated = {}
stamp = root / '.buck2/sync-stamp.json'
if stamp.is_file():
    for part, saved in json.loads(stamp.read_text())['parts'].items():
        reads = saved['reads']
        current = dict(files={}, probes={}, globs={}, outputs={})
        for name, old in reads['files'].items():
            path = root / name
            file(path)
            current['files'][name] = dict(saved=old, live=files.get(str(path)))
        for name, old in reads['probes'].items():
            path = root / name
            live = path.is_file()
            current['probes'][name] = dict(saved=old, live=live)
            file(path)
        for directory, patterns in reads['globs'].items():
            current['globs'][directory] = {}
            for pattern, old in patterns.items():
                # Same glob evaluation as generated.Reads.glob; newly matched
                # files and symlink directory contents are hashed right now.
                live = sorted((root / directory).glob(pattern))
                current['globs'][directory][pattern] = dict(saved=old, live=[str(p) for p in live])
                for path in live:
                    tree(path)
        for name, old in saved['outputs'].items():
            path = root / name
            file(path)
            current['outputs'][name] = dict(saved=old, live=files.get(str(path)))
        evaluated[part] = current

env = {k: v for k, v in os.environ.items() if k.startswith(('HIRSEL_', 'LLVM_', 'SPECTRAL_AUDIT_')) or k in ['PATH', 'LD_LIBRARY_PATH']}
for key in ['HIRSEL_IMAGE', 'HIRSEL_RUNTIME_ARCHIVES', 'HIRSEL_LINK_KIT', 'HIRSEL_GLIBC_FLOOR', 'LLVM_SYS_231_PREFIX']:
    if env.get(key):
        tree(env[key])
for path in sys.argv[3:]:
    tree(path)
for name in ['tools/buck2/toolchains/rust-files', 'tools/buck2/toolchains/llvm-files']:
    tree(root / name)
for key, value in env.items():
    if key.startswith('HIRSEL_TOOLCHAIN_') or key == 'HIRSEL_NATIVE_ENVIRONMENT':
        tree(value)
file(sys.executable)
(out / 'seal.json').write_text(json.dumps(dict(cwd=str(root), head=head, source_scope=scope,
    environment=env, files=files, directories=directories, cycle_edges=cycles,
    evaluated_generation=evaluated, timing='before consumer; live evaluated reads/probes/globs/outputs'),
    sort_keys=True, indent=2) + '\n')
print('SPECTRAL_SEAL', out, len(files), flush=True)
