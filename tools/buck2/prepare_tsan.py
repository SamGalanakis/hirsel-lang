#!/usr/bin/env python3
"""Prepare instrumented std for Buck2's opt-in thread sanitizer platform.

Run under tools/capped. This builds std only; the runtime, dependencies and
8-worker control are compiled and tested by Buck2. Nothing is installed in
the normal toolchain, shared store, or a different checkout.
"""
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[2]

def main():
    version = json.loads((ROOT / 'tools/buck2/toolchain-lock.json').read_text())['rust_version']
    triple = 'x86_64-unknown-linux-gnu'
    installed = subprocess.check_output(['rustc', '+' + version, '--version'], text=True)
    pinned = subprocess.check_output([ROOT / 'tools/buck2/toolchains/rust-files/bin/rustc', '--version'], text=True)
    if installed != pinned:
        raise SystemExit('rustup compiler differs from the Buck2 compiler; prepare the pinned toolchain first')
    subprocess.run(['rustup', 'component', 'add', 'rust-src', '--toolchain', version], check=True)
    with tempfile.TemporaryDirectory(prefix='hirsel-tsan-std-') as scratch:
        project = Path(scratch)
        (project / 'src').mkdir()
        (project / 'Cargo.toml').write_text('[package]\nname="hirsel-tsan-std"\nversion="0.0.0"\nedition="2024"\n[workspace]\n')
        (project / 'src/lib.rs').write_text('pub fn bootstrap() {}\n')
        subprocess.run(['cargo', '+' + version, 'build', '-Zbuild-std=std,panic_unwind,test',
                        '--target', triple, '--manifest-path', str(project / 'Cargo.toml'), '-j', '2'],
                       env=os.environ | {'RUSTC_BOOTSTRAP': '1', 'RUSTFLAGS': '-Zsanitizer=thread -Copt-level=1'}, check=True)
        destination = ROOT / 'tools/buck2/toolchains/tsan-files'
        staged = destination.with_suffix('.new')
        staged.mkdir(exist_ok=False)
        try:
            for file in (project / 'target' / triple / 'debug/deps').glob('*.rlib'):
                if not file.name.startswith('libhirsel_tsan_std-'):
                    shutil.copy2(file, staged / file.name)
            # One compiler version per private checkout; replace only our own generated std.
            if destination.exists():
                shutil.rmtree(destination)
            staged.rename(destination)
        finally:
            if staged.exists():
                shutil.rmtree(staged)

if __name__ == '__main__':
    main()
