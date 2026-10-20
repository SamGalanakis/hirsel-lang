"""The repository's generated files, by who writes them. No lane commits one,
so no rebase can conflict on one.

- BUILD_INPUTS: what sync.py derives from the Cargo workspace and the committed
  measurements and pins. They are gitignored and generated on demand:
  bootstrap.py writes them, and every entry point that runs Buck2 (prepare.py,
  tools/gate, tools/land, tools/bless.sh) regenerates them when the tree they
  were generated from changed (`sync.py --if-stale`). Without them a bare
  `.buck2/bin/buck2` reads the committed BUCK.unsynced stubs and fails with
  one line saying to run sync.py.
- LAND_WRITTEN: generated pages people read in the repository. They stay
  committed, and only tools/land writes them: it regenerates them on the
  batch's tip and pushes them with the batch. tools/gate and tools/land refuse
  a lane change that touches one.

The stamp records the tree the build inputs were generated from: HEAD's tree
and every path `git status` reports, with its content. Generation reads only
the tree (and the tools the tree pins), so an unchanged stamp means unchanged
outputs.
"""

from __future__ import annotations

import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys

BUILD_INPUTS = (
    'BUCK',
    'third-party/BUCK',
    'third-party/Cargo.toml',
    'third-party/Cargo.lock',
    'third-party/lib.rs',
    'tools/buck2/exec_sizes.bzl',
    'tools/buck2/test_sizes.bzl',
    'tools/buck2/workspace-metadata.json',
    'tools/native-toolchain/pins.bzl',
)
LAND_WRITTEN = ('docs/api.md',)
STAMP = '.buck2/sync-stamp.json'
SYNC_HINT = 'run python3 tools/buck2/sync.py'


def git(root: Path, *arguments: str) -> bytes:
    return subprocess.run(['git', *arguments], cwd=root, check=True, capture_output=True).stdout


def tree_state(root: Path) -> str:
    """A digest of the tree the build inputs derive from: HEAD's tree, and each
    changed, deleted or untracked path that git reports, with its content.
    The outputs and ignored files (Buck2's state) are not part of it."""
    digest = hashlib.sha256()
    head = subprocess.run(['git', 'rev-parse', '-q', '--verify', 'HEAD^{tree}'], cwd=root,
                          capture_output=True, text=True).stdout.strip()
    digest.update(head.encode() + b'\0')
    status = git(root, 'status', '--porcelain=v1', '-z', '--untracked-files=all', '--no-renames')
    for entry in sorted(path for path in status.split(b'\0') if path):
        if os.fsdecode(entry[3:]) in BUILD_INPUTS:
            continue  # an output, even while its removal from git is uncommitted
        path = root / os.fsdecode(entry[3:])
        digest.update(entry + b'\0')
        if path.is_file() and not path.is_symlink():
            digest.update(hashlib.sha256(path.read_bytes()).digest())
    return digest.hexdigest()


def outputs(root: Path) -> dict[str, str]:
    """Each build input's content digest, or '' when it is missing."""
    return {path: hashlib.sha256((root / path).read_bytes()).hexdigest() if (root / path).is_file() else ''
            for path in BUILD_INPUTS}


def record(root: Path, state: str) -> None:
    """Note that the build inputs were generated from the tree with `state`."""
    stamp = root / STAMP
    stamp.parent.mkdir(exist_ok=True)
    pending = stamp.with_name(f'.{stamp.name}.{os.getpid()}')
    pending.write_text(json.dumps({'tree': state, 'outputs': outputs(root)}, indent=1, sort_keys=True) + '\n')
    pending.replace(stamp)


def current(root: Path) -> bool:
    """Whether every build input exists, unedited, as generated from this tree."""
    try:
        stamp = json.loads((root / STAMP).read_text())
    except (OSError, ValueError):
        return False
    found = outputs(root)
    return all(found.values()) and stamp.get('outputs') == found and stamp.get('tree') == tree_state(root)


def ensure(root: Path) -> subprocess.CompletedProcess | None:
    """Regenerate the build inputs when they are stale: None when they were
    current, otherwise sync.py's result."""
    if current(root):
        return None
    return subprocess.run([sys.executable, str(root / 'tools/buck2/sync.py')], cwd=root,
                          capture_output=True, text=True, env=os.environ | {'HIRSEL_LAND_TREE': str(root)})


def lane_written(paths) -> list[str]:
    """The generated files among the paths a lane change writes (adds or
    edits): no lane commits them."""
    return sorted(path for path in paths if path in LAND_WRITTEN or path in BUILD_INPUTS)


def refusal(paths) -> list[str]:
    """Why a lane change touching generated files is refused, with how to drop
    them; empty when it touches none."""
    touched = lane_written(paths)
    if not touched:
        return []
    pages = [path for path in touched if path in LAND_WRITTEN]
    inputs = [path for path in touched if path in BUILD_INPUTS]
    return [f'the change touches generated files, which no lane commits: {", ".join(touched)}',
            'tools/land regenerates docs/api.md on the batch it pushes, and sync.py regenerates the '
            'gitignored build files on demand. Drop them from the change:',
            *([f'  git checkout $(git merge-base HEAD origin/main) -- {" ".join(pages)}'] if pages else []),
            *([f'  git rm -q --cached {" ".join(inputs)}'] if inputs else []),
            '  git commit --amend --no-edit',
            'then tools/gate --extend (it reruns nothing for the dropped build files), then tools/land' if inputs else
            'docs/api.md is not part of the gated identity, so tools/land accepts the change again without a new gate.']
