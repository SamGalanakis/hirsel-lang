#!/usr/bin/env python3
"""Retain the actual public-fixture compiler/std/runtime/linker/helper closure.

Runs as setup on the selected host executor before any bounded cell starts.
Uses hardlinks only to immutable build artifacts; source is copied by value.
"""
import hashlib
import json
import os
from pathlib import Path
import shutil
import sys

out, crate, harness, cli = map(Path, sys.argv[1:])
repo = crate.parent.parent
out = out.resolve()
assert not out.is_relative_to(repo.resolve()), 'evidence must be outside repo'
closure = out / 'closure'
closure.mkdir()
roots = {'harness': harness, 'cli': cli, 'stdlib': repo/'stdlib/std@1'}
for name in ('HIRSEL_IMAGE', 'HIRSEL_RUNTIME_ARCHIVES', 'LLVM_SYS_231_PREFIX', 'HIRSEL_LINK_KIT', 'HIRSEL_GLIBC_FLOOR'):
    if name in os.environ:
        roots[name] = Path(os.environ[name])
if (out/'qualified.image').exists():
    roots['HIRSEL_IMAGE'] = out/'qualified.image'
manifest = []
for name, original in roots.items():
    original = original.resolve(strict=True)
    files = [original] if original.is_file() else sorted(p for p in original.rglob('*') if p.is_file())
    for source in files:
        relative = Path(source.name) if original.is_file() else source.relative_to(original)
        destination = closure/name/relative
        destination.parent.mkdir(parents=True, exist_ok=True)
        if name == 'stdlib':
            shutil.copy2(source, destination)
        else:
            try:
                os.link(source.resolve(), destination)
            except OSError:
                shutil.copy2(source, destination)
        h = hashlib.sha256()
        with destination.open('rb') as stream:
            for chunk in iter(lambda:stream.read(1024*1024), b''):
                h.update(chunk)
        manifest.append(dict(name=name,path=str(source),retained=str(destination),sha256=h.hexdigest(),bytes=destination.stat().st_size))
(out/'actual-inputs.json').write_text(json.dumps(manifest,indent=2)+'\n')
(out/'effective-env.json').write_text(json.dumps({k:v for k,v in os.environ.items() if k.startswith(('HIRSEL_','LLVM_','POLICY_EVALUATOR_33_'))},indent=2)+'\n')
print(f'retained {len(manifest)} actual inputs before bounded cells')
