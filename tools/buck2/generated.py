"""The repository's generated files, by who writes them. No lane commits one,
so no rebase can conflict on one.

- BUILD_INPUTS: what sync.py derives from the Cargo workspace and the committed
  measurements and pins. They are gitignored and generated on demand:
  bootstrap.py writes them, and every entry point that runs Buck2 (prepare.py,
  tools/land, tools/main-runs, tools/bless.sh) regenerates the parts whose
  inputs changed since they were generated (`sync.py --if-stale`). Without them a bare
  `.buck2/bin/buck2` reads the committed BUCK.unsynced stubs and fails with
  one line saying to run sync.py.
- MAIN_WRITTEN: generated pages people read in the repository. They stay
  committed, and only main's hourly run (tools/main-runs) writes them: it
  regenerates them and pushes a change to main. tools/land refuses a lane
  change that touches one.

The stamp records, for each part of the build inputs, what its generation
read (`Reads`: file contents, existence tests and glob matches, including the
generator's own code) and the outputs it wrote. A part is stale when an output
is missing or edited or when anything it read reads differently, so an edit
to a file generation never reads (documentation, a corpus program) regenerates
nothing, and generation writes an output only when its bytes change.
"""

from __future__ import annotations

import hashlib
import json
import os
from pathlib import Path
import subprocess
import tempfile
import sys

# Each part's outputs. Third-party dependency generation (Cargo metadata, then
# Reindeer) and first-party inventory generation (Cargo metadata, then source
# inspection) are stamped apart, so neither reruns for the other's inputs.
PARTS = {
    'third-party': ('third-party/BUCK', 'third-party/Cargo.toml', 'third-party/Cargo.lock', 'third-party/lib.rs'),
    'first-party': ('BUCK', 'tools/buck2/exec_sizes.bzl', 'tools/buck2/test_sizes.bzl',
                    'tools/buck2/workspace-metadata.json', 'tools/native-toolchain/pins.bzl'),
}
BUILD_INPUTS = tuple(sorted(path for outputs in PARTS.values() for path in outputs))
MAIN_WRITTEN = ('docs/api.md',)
STAMP = '.buck2/sync-stamp.json'
SYNC_HINT = 'run python3 tools/buck2/sync.py'


CHANGED = 'changed during generation'


def digest(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


class Reads:
    """What one part's generation read: each file's content digest, each path
    whose existence alone it tested, and each glob's matches. Generation reads
    the tree only through these methods, so the record is everything its
    outputs depend on, ignored files included."""

    def __init__(self, root: Path):
        self.root = root.resolve()
        self.files: dict[str, str] = {}
        self.probes: dict[str, bool] = {}
        self.globs: dict[str, dict[str, list[str]]] = {}

    def key(self, path) -> str:
        return key(self.root, path)

    def bytes(self, path) -> bytes:
        data = Path(path).read_bytes()
        self.note(self.files, self.key(path), digest(data))
        return data

    @staticmethod
    def note(record: dict, name: str, value) -> None:
        """Record what a read saw. One that reads differently a second time in
        the same generation changed while it ran: the record then matches
        nothing, so the part stays stale."""
        record[name] = value if record.get(name, value) == value else CHANGED

    def text(self, path, errors: str = 'strict') -> str:
        """The file as `Path.read_text` reads it: UTF-8 with universal newlines."""
        text = self.bytes(path).decode('utf-8', errors)
        return text.replace('\r\n', '\n').replace('\r', '\n')

    def is_file(self, path) -> bool:
        found = Path(path).is_file()
        self.note(self.probes, self.key(path), found)
        return found

    def glob(self, directory, pattern: str) -> list[Path]:
        matches = sorted(Path(directory).glob(pattern))
        self.note(self.globs.setdefault(self.key(directory), {}), pattern, [self.key(match) for match in matches])
        return matches

    def record(self) -> dict:
        return {'files': self.files, 'probes': self.probes, 'globs': self.globs}


def key(root: Path, path) -> str:
    """A path as a stamp names it: relative to the root, or absolute outside it."""
    text, prefix = os.fspath(path), os.fspath(root)
    if text == prefix:
        return '.'
    return text[len(prefix) + 1:] if text.startswith(prefix + os.sep) else text


def unchanged(root: Path, reads: dict) -> bool:
    """Whether every file, probe and glob a part's generation read reads the same now."""
    root = root.resolve()
    for path, recorded in reads['files'].items():
        file = root / path
        if not file.is_file() or digest(file.read_bytes()) != recorded:
            return False
    if any((root / path).is_file() != found for path, found in reads['probes'].items()):
        return False
    return all([key(root, match) for match in sorted((root / directory).glob(pattern))] == matches
               for directory, patterns in reads['globs'].items() for pattern, matches in patterns.items())


def outputs(root: Path, part: str) -> dict[str, str]:
    """Each of a part's outputs' content digest, or '' when it is missing."""
    return {path: digest((root / path).read_bytes()) if (root / path).is_file() else '' for path in PARTS[part]}


def load(root: Path) -> dict:
    try:
        stamp = json.loads((root / STAMP).read_text())
    except (OSError, ValueError):
        return {}
    return stamp.get('parts', {}) if isinstance(stamp, dict) else {}


def record(root: Path, part: str, reads: Reads) -> None:
    """Note that a part's outputs, as they are now, were generated from `reads`."""
    stamp = root / STAMP
    stamp.parent.mkdir(exist_ok=True)
    parts = load(root) | {part: {'reads': reads.record(), 'outputs': outputs(root, part)}}
    with tempfile.NamedTemporaryFile(mode='w', dir=stamp.parent, prefix=f'.{stamp.name}.', delete=False) as output:
        pending = Path(output.name)
        try:
            output.write(json.dumps({'parts': parts}, indent=1, sort_keys=True) + '\n')
            output.close()
            pending.replace(stamp)
        finally:
            pending.unlink(missing_ok=True)


def stale(root: Path) -> list[str]:
    """The parts whose outputs are missing, edited, or generated from inputs
    that read differently now."""
    parts = load(root)
    found = []
    for part in PARTS:
        entry = parts.get(part)
        current_outputs = outputs(root, part)
        if not (entry and all(current_outputs.values()) and entry.get('outputs') == current_outputs
                and unchanged(root, entry['reads'])):
            found.append(part)
    return found


def current(root: Path) -> bool:
    """Whether every build input exists, unedited, as generated from what is there now."""
    return not stale(root)


def ensure(root: Path) -> subprocess.CompletedProcess | None:
    """Regenerate the build inputs when they are stale: None when they were
    current, otherwise sync.py's result."""
    if current(root):
        return None
    return subprocess.run([sys.executable, str(root / 'tools/buck2/sync.py'), '--if-stale'], cwd=root,
                          capture_output=True, text=True)


def require(root: Path) -> None:
    """Regenerate stale build inputs, or exit with sync.py's last lines."""
    synced = ensure(root)
    if synced is not None and synced.returncode:
        lines = [line for line in (synced.stdout + synced.stderr).splitlines() if line.strip()]
        raise SystemExit('cannot generate the Buck2 build files (tools/buck2/sync.py)\n' + '\n'.join(lines[-20:]))


def lane_written(paths) -> list[str]:
    """The generated files among the paths a lane change writes (adds or
    edits): no lane commits them."""
    return sorted(path for path in paths if path in MAIN_WRITTEN or path in BUILD_INPUTS)


def refusal(paths) -> list[str]:
    """Why a lane change touching generated files is refused, with how to drop
    them; empty when it touches none."""
    touched = lane_written(paths)
    if not touched:
        return []
    pages = [path for path in touched if path in MAIN_WRITTEN]
    inputs = [path for path in touched if path in BUILD_INPUTS]
    return [f'the change touches generated files, which no lane commits: {", ".join(touched)}',
            'main\'s hourly run (tools/main-runs) regenerates docs/api.md, and sync.py regenerates the '
            'gitignored build files on demand. Drop them from the change:',
            *([f'  git checkout $(git merge-base HEAD origin/main) -- {" ".join(pages)}'] if pages else []),
            *([f'  git rm -q --cached {" ".join(inputs)}'] if inputs else []),
            '  git commit --amend --no-edit',
            'then tools/land.']
