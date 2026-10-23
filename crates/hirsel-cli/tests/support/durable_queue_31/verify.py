"""Full entry native qualification. Raw artifacts retained by the Rust harness."""
import argparse
import json
import os
from pathlib import Path
import subprocess

p = argparse.ArgumentParser()
p.add_argument('--hirsel', required=True)
p.add_argument('--root', type=Path, required=True)
a = p.parse_args()
for backend in ['llvm', 'cranelift']:
    binary = a.root / backend
    built = subprocess.run([a.hirsel, 'build', '--backend', backend, str(a.root / 'hirsel.json'), '-o', str(binary)], capture_output=True)
    (a.root / (backend + '-build.stdout')).write_bytes(built.stdout)
    (a.root / (backend + '-build.stderr')).write_bytes(built.stderr)
    assert built.returncode == 0, (backend, built.stderr.decode())
host = subprocess.run(['python3', str(a.root / 'host.py'), '--hirsel', a.hirsel, '--root', str(a.root), '--capped', str(a.root / 'capped')], capture_output=True)
(a.root / 'host.stdout').write_bytes(host.stdout)
(a.root / 'host.stderr').write_bytes(host.stderr)
assert host.returncode == 0, host.stderr.decode()
print(host.stdout.decode())
