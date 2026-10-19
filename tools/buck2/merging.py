"""How tools/land resolves a rebase conflict in a generated or append-only file
instead of stopping: lanes regenerate or append to these files, so two
changes touching them conflict without disagreeing.

- docs/api.md and the root BUCK are generated: land keeps main's side and
  regenerates them on the rebased tree (the index when it pushes, BUCK with
  sync.py after the change's commits), so their merge is never hand-made;
- third-party/NOTICES.md is append-only: hunks where both sides only add
  lines keep main's lines, then the change's;
- a package manifest (`hirsel.json`) merges structurally: keys, sources and
  dependencies either side added or removed, so two lanes adding std modules
  both keep theirs.

Anything else, or a hunk that both sides edit differently, stays a conflict.
"""

from __future__ import annotations

import json
import os
import subprocess
import tempfile

REGENERATED = ('docs/api.md', 'BUCK')
APPEND_ONLY = ('third-party/NOTICES.md',)
MANIFEST = 'hirsel.json'


class Conflict(Exception):
    """Both sides changed the same part differently."""


def resolvable(path: str) -> bool:
    return path in REGENERATED or path in APPEND_ONLY or path.rpartition('/')[2] == MANIFEST


def resolve(path: str, base: str | None, ours: str | None, theirs: str | None) -> str | None:
    """The merged text of a conflicted path, from its merge base and the two
    sides (`ours` is main's side in a rebase, `theirs` the change's), or None
    when it stays a conflict. A missing side (a delete) is never resolved."""
    if ours is None or theirs is None or not resolvable(path):
        return None
    if path in REGENERATED:
        return ours
    try:
        if path in APPEND_ONLY:
            return union(base or '', ours, theirs)
        return manifest(base, ours, theirs)
    except Conflict:
        return None


def union(base: str, ours: str, theirs: str) -> str:
    """A three-way line merge whose conflicting hunks are additions on both
    sides: main's added lines, then the change's."""
    with tempfile.TemporaryDirectory() as scratch:
        names = []
        for name, text in (('ours', ours), ('base', base), ('theirs', theirs)):
            path = os.path.join(scratch, name)
            with open(path, 'w') as file:
                file.write(text)
            names.append(path)
        merged = subprocess.run(['git', 'merge-file', '-p', '--diff3', '-L', 'ours', '-L', 'base', '-L', 'theirs',
                                 *names], capture_output=True, text=True)
    if merged.returncode < 0 or merged.returncode > 127:
        raise Conflict(merged.stderr)
    out, part, sides = [], None, {'ours': [], 'base': [], 'theirs': []}
    for line in merged.stdout.splitlines(keepends=True):
        if line.startswith('<<<<<<< ours'):
            part = 'ours'
        elif part == 'ours' and line.startswith('||||||| base'):
            part = 'base'
        elif part == 'base' and line.startswith('======='):
            part = 'theirs'
        elif part == 'theirs' and line.startswith('>>>>>>> theirs'):
            if sides['base']:
                raise Conflict('both sides edit the same lines')
            out += sides['ours'] + sides['theirs']
            part, sides = None, {'ours': [], 'base': [], 'theirs': []}
        elif part is None:
            out.append(line)
        else:
            sides[part].append(line)
    if part is not None:
        raise Conflict('unterminated conflict')
    return ''.join(out)


def manifest(base: str | None, ours: str, theirs: str) -> str:
    """A structural three-way merge of a package manifest, written as the
    repository writes them (two-space JSON); a side that is not in that form
    stays a conflict rather than being reformatted."""
    values = []
    for text in (ours, theirs):
        value = json.loads(text)
        if canonical(value) != text:
            raise Conflict('not in canonical form')
        values.append(value)
    return canonical(merge(json.loads(base) if base else None, *values))


def canonical(value) -> str:
    return json.dumps(value, indent=2, ensure_ascii=False) + '\n'


MISSING = object()


def merge(base, ours, theirs):
    if ours == theirs:
        return ours
    if base == ours:
        return theirs
    if base == theirs:
        return ours
    if isinstance(ours, dict) and isinstance(theirs, dict):
        base = base if isinstance(base, dict) else {}
        merged = {}
        for key in list(ours) + [key for key in theirs if key not in ours]:
            value = merge(base.get(key, MISSING), ours.get(key, MISSING), theirs.get(key, MISSING))
            if value is not MISSING:
                merged[key] = value
        return merged
    if isinstance(ours, list) and isinstance(theirs, list):
        base = base if isinstance(base, list) else []
        # Entries either side removed go; entries either side added stay,
        # main's first. Both sides replacing one entry is an edit conflict.
        added = [entry for entry in theirs if entry not in ours and entry not in base]
        if (any(entry not in ours and entry not in theirs for entry in base)
                and added and any(entry not in base for entry in ours)):
            raise Conflict('both sides replaced the same entry')
        return [entry for entry in ours if entry in theirs or entry not in base] + added
    raise Conflict('both sides changed the same value')
