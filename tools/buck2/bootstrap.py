#!/usr/bin/env python3
"""Install the pinned Buck2 client and every pinned build input into ignored state.

`.buck2/bin/buck2` is the official release named in `pins.json`; the prelude at
`.buck2/prelude` is the one bundled in that executable, with
prelude_overlay.py's checksum-pinned patches. The Rust toolchain, the LLVM tree and
Reindeer come from their own bootstrap scripts. Pinned inputs come from the
shared store in bootstrap_store.py when it is usable, and are otherwise
downloaded into this checkout alone. Prints the client's path.
"""
import argparse
from contextlib import contextmanager
import fcntl
import hashlib
import json
import os
from pathlib import Path
import shutil
import stat
import subprocess
import sys
import tempfile
import urllib.request

import bootstrap_store

ROOT = Path(__file__).resolve().parents[2]
PINS = json.loads(Path(__file__).with_name('pins.json').read_text())
PIN = PINS['buck2']
OVERLAY = ROOT / 'tools/buck2/prelude_overlay.py'
TOOLS = ('bootstrap_rust_toolchain.py', 'bootstrap_llvm.py', 'bootstrap_reindeer.py')


def digest(path):
    with path.open('rb') as source:
        return hashlib.file_digest(source, 'sha256').hexdigest()


@contextmanager
def preparation_lock(state):
    if state.is_symlink():
        raise SystemExit('Refusing a symlinked .buck2 state directory')
    state.mkdir(exist_ok=True)
    descriptor = os.open(state / 'prepare.lock', os.O_CREAT | os.O_RDWR | os.O_NOFOLLOW, 0o600)
    try:
        if not stat.S_ISREG(os.fstat(descriptor).st_mode):
            raise SystemExit('Buck2 preparation lock must be a regular file')
        fcntl.flock(descriptor, fcntl.LOCK_EX)
        yield descriptor
    finally:
        os.close(descriptor)


def prepare_prelude(executable, lock_fd):
    """Install the overlaid prelude unless it is already in place; returns whether it changed."""
    overlay = [sys.executable, str(OVERLAY), '--buck2', str(executable)]
    if subprocess.run(overlay + ['--check'], cwd=ROOT, capture_output=True).returncode == 0:
        return False
    prelude = ROOT / '.buck2/prelude'
    if prelude.is_dir() and subprocess.run(overlay, cwd=ROOT, capture_output=True).returncode != 0:
        # Derived state from another overlay revision: rebuild it from the
        # pinned executable, whose stock files the guards then check.
        shutil.rmtree(prelude)
    if not prelude.is_dir():
        def build(tree):
            # Buck2 expands its bundled prelude only at the cell's own path.
            subprocess.run(overlay, check=True, cwd=ROOT, pass_fds=(lock_fd,))
            shutil.move(prelude, tree)

        identity = {'buck2': PIN['executable_sha256'], 'overlay': digest(OVERLAY), 'pins': PINS['prelude_overlay']}
        bootstrap_store.materialize(ROOT, 'prelude', identity, build, prelude)
    subprocess.run(overlay, check=True, cwd=ROOT, pass_fds=(lock_fd,))
    return True


def prepare_tools(executable, lock_fd):
    replaced = prepare_prelude(executable, lock_fd)
    for name in TOOLS:
        script = [sys.executable, str(ROOT / 'tools/buck2' / name)]
        if subprocess.run(script + ['--check'], cwd=ROOT, capture_output=True).returncode != 0:
            subprocess.run(script, check=True, cwd=ROOT, stdout=subprocess.DEVNULL, pass_fds=(lock_fd,))
            replaced = True
    if replaced:
        # A running daemon's file watcher can miss a replaced tree's new files.
        subprocess.run([str(executable), 'kill'], cwd=ROOT, capture_output=True)


def prepare_client(args, state, lock_fd):
    destination = state / 'bin'
    if destination.is_symlink():
        raise SystemExit('Refusing a symlinked executable directory')
    destination.mkdir(exist_ok=True)
    executable = destination / 'buck2'
    if not (executable.is_file() and not executable.is_symlink() and digest(executable) == PIN['executable_sha256']):
        def fetch(tree):
            tree.mkdir()
            archive = tree.parent / 'buck2.zst'
            with urllib.request.urlopen(PIN['url'], timeout=60) as source, archive.open('wb') as output:
                shutil.copyfileobj(source, output)
            if digest(archive) != PIN['archive_sha256']:
                raise SystemExit('Buck2 release archive checksum mismatch')
            binary = tree / 'buck2'
            subprocess.run(['zstd', '-q', '-d', str(archive), '-o', str(binary)], check=True, pass_fds=(lock_fd,))
            archive.unlink()
            if digest(binary) != PIN['executable_sha256']:
                raise SystemExit('Buck2 executable checksum mismatch')
            binary.chmod(0o755)

        with tempfile.TemporaryDirectory(prefix='bootstrap-', dir=state) as work:
            tree = Path(work) / 'client'
            identity = {key: PIN[key] for key in ('archive_sha256', 'executable_sha256')}
            if not bootstrap_store.materialize(ROOT, 'buck2', identity, fetch, tree):
                fetch(tree)
            binary = tree / 'buck2'
            if digest(binary) != PIN['executable_sha256']:
                raise SystemExit('Buck2 executable checksum mismatch')
            os.replace(binary, executable)
    if not args.client_only:
        prepare_tools(executable, lock_fd)
    print(executable)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--client-only', action='store_true', help='Prepare only the official client.')
    parser.add_argument('--prune-store', action='store_true', help='Delete shared store entries that no existing checkout references and nothing used recently, then exit.')
    parser.add_argument('--unused-for', type=float, default=86400, metavar='SECONDS', help='With --prune-store, also keep an entry used this recently (default: one day).')
    parser.add_argument('--verify-store', action='store_true', help='Rehash every shared store entry, delete the corrupt ones, then exit.')
    args = parser.parse_args()
    if args.prune_store:
        raise SystemExit(bootstrap_store.prune(args.unused_for))
    if args.verify_store:
        raise SystemExit(bootstrap_store.verify())
    state = ROOT / '.buck2'
    with preparation_lock(state) as lock_fd:
        prepare_client(args, state, lock_fd)


if __name__ == '__main__':
    main()
