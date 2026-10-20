#!/usr/bin/env python3
"""Run the open resource-return repro without adding a failing corpus case.

Pass the built hirsel-conformance executable. A current bug returns 1 and a
checked-trap diagnostic; after a fix the ordinary conformance verdict is 0.
This is a reproducer, not a passing test or a known-failure exclusion.
"""
import json
from pathlib import Path
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[4]
SOURCE = Path(__file__).with_name('resource-remove-run.hir')

if len(sys.argv) != 2:
    sys.exit('usage: check-resource.py /absolute/path/to/hirsel_conformance')
with tempfile.TemporaryDirectory(prefix='facade-resource-') as directory:
    root = Path(directory)
    (root / 'docs').symlink_to(ROOT / 'docs', target_is_directory=True)
    corpus = root / 'conformance/DROP'
    corpus.mkdir(parents=True)
    (root / 'conformance/passing.txt').write_text('')
    (root / 'conformance/m2-runtime.txt').write_text('')
    target = corpus / 'DROP-2-facade-resource-remove-run.hir'
    target.write_text(SOURCE.read_text())
    manifest = json.loads(SOURCE.with_suffix('.hir.json').read_text())
    manifest['sources'][0]['path'] = target.name
    manifest['dependencies'][0]['manifest'] = str(SOURCE.parent.parent / 'hirsel.json')
    target.with_suffix('.hir.json').write_text(json.dumps(manifest))
    result = subprocess.run([str(Path(sys.argv[1]).resolve()), '--root', str(root),
                             '--path', 'conformance/DROP', '--json'])
    sys.exit(result.returncode)
