#!/usr/bin/env python3
"""Materialize committed host tapes and build their native replay consumers."""
import argparse
import json
from pathlib import Path
import shutil
import subprocess
import sys

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('family', choices=['toy_analyzer_31', 'sheet_eval_31', 'durable_queue_31'])
parser.add_argument('--hirsel', type=Path, required=True)
parser.add_argument('--output', type=Path, required=True)
args = parser.parse_args()
repo = Path(__file__).resolve().parents[4]
source = Path(__file__).resolve().parent / args.family / 'current-tapes'
shutil.copytree(source, args.output)
root = args.output.resolve()
shutil.copyfile(repo / 'tools/capped', root / 'capped')
(root / 'capped').chmod(0o755)
for manifest in root.rglob('hirsel.json'):
    data = json.loads(manifest.read_text())
    for dependency in data.get('dependencies', []):
        if dependency['package']['name'] == 'std':
            dependency['manifest'] = str(repo / 'stdlib/std@1/hirsel.json')
    manifest.write_text(json.dumps(data, indent=2) + '\n')
sheet = args.family == 'sheet_eval_31'
manifest = root / ('qualified-app/hirsel.json' if sheet else 'hirsel.json')
for backend in ['llvm', 'cranelift']:
    binary = root / (f'app-{backend}' if sheet else backend)
    subprocess.run([sys.executable, str(root / 'capped'), '--timeout', '300',
                    str(args.hirsel.resolve()), 'build', str(manifest),
                    '--backend', backend, '-o', str(binary)], check=True)
print(root)
