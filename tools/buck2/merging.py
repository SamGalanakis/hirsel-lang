"""How a rebase merges the files many lanes edit at once instead of stopping:
two changes that each add to one of them conflict without disagreeing.

Git runs this as the `hirsel` merge driver for the paths .gitattributes names
(bootstrap.py installs the driver in the clone's config), so a lane's own
rebase and tools/land's batch rebase merge them alike; tools/land also
resolves a conflict in one with `resolve` where the driver is not installed.

- third-party/NOTICES.md is append-only: hunks where both sides only add
  lines keep main's lines, then the change's;
- a package manifest (`hirsel.json`) merges structurally: keys, sources and
  dependencies either side added or removed, so two lanes adding std modules
  both keep theirs;
- independent line ledgers and module declarations use a three-way set merge:
  either side's removal wins and both sides' additions survive;
- corpus path lists merge as sets, accepting unsorted appends and standalone
  comments while preserving removals and conflicting expectation changes;
- a sorted list (SORTED_LISTS: ratchet and expectation lists, one entry per
  line in the order of its key, the text before a tab or ` | `): where both
  sides change one stretch, the entries either side added stay, the entries
  either side removed go, and the stretch is written in key order. Two entries
  with one key (both sides changed one entry), a stretch out of order, or a
  comment or blank line in it stays a conflict. Each line of these lists
  stands alone (a path, or one declaration's blessed signature), so the merge
  is what regenerating the list on the merged tree writes.

Anything else, or a hunk that both sides edit differently, stays a conflict:
the driver then leaves git's usual conflict markers.
"""

from __future__ import annotations

from fnmatch import fnmatchcase
import json
import os
import subprocess
import sys
import tempfile

APPEND_ONLY = ('third-party/NOTICES.md',)
MANIFEST = 'hirsel.json'
SORTED_LISTS = (
    'conformance/passing.txt',
    'conformance/pending.txt',
    'crates/hirsel-engine/src/registry/review-table.txt',
    'crates/hirsel-authority/src/corpus/passing.txt',
    'crates/hirsel-authority/src/corpus/pending.txt',
    'crates/hirsel-ownership/src/tests/passing.txt',
    'crates/hirsel-ownership/src/tests/pending.txt',
    'native-tests/library/operations.tsv',
    'native-tests/regressions/*.txt',
)
# Corpus ratchets are path sets even when lanes append unsorted batches or
# comments. A path's inline reason is part of its value, not its identity.
PATH_LISTS = tuple(path for path in SORTED_LISTS if path.endswith(('passing.txt', 'pending.txt')))
# Whole-line identities, without key or ordering semantics. Keep this in sync
# with the independent-line patterns in .gitattributes.
LINE_SETS = ('crates/*/tests/*/modules.rs', 'tools/known-failures.tsv')
DRIVER = 'hirsel'


class Conflict(Exception):
    """Both sides changed the same part differently."""


def sorted_list(path: str) -> bool:
    return any(fnmatchcase(path, pattern) for pattern in SORTED_LISTS)


def line_set(path: str) -> bool:
    return any(fnmatchcase(path, pattern) for pattern in LINE_SETS)


def resolvable(path: str) -> bool:
    return line_set(path) or path in APPEND_ONLY or path.rpartition('/')[2] == MANIFEST or sorted_list(path)


def resolve(path: str, base: str | None, ours: str | None, theirs: str | None) -> str | None:
    """The merged text of a path, from its merge base and the two sides
    (`ours` is main's side in a rebase, `theirs` the change's), or None when it
    stays a conflict. A missing side (a delete) is never resolved."""
    if ours is None or theirs is None or not resolvable(path):
        return None
    try:
        if line_set(path):
            return merge_line_set(base or '', ours, theirs)
        if path in APPEND_ONLY:
            return line_merge(base or '', ours, theirs, appended)
        if path in PATH_LISTS:
            return path_list(base or '', ours, theirs)
        if sorted_list(path):
            return line_merge(base or '', ours, theirs, sorted_stretch)
        return manifest(base, ours, theirs)
    except Conflict:
        return None


def merge_line_set(base: str, ours: str, theirs: str) -> str:
    """(base & ours & theirs) | (ours - base) | (theirs - base).

    Use the removal rule from sorted_stretch over the whole file: Git's hunk
    boundaries must not decide whether a deleted row returns. Keep ours' order,
    then theirs' new lines; these lists have no ordering semantics.
    """
    ancestor, left, right = (text.splitlines() for text in (base, ours, theirs))
    removed = (set(ancestor) - set(left)) | (set(ancestor) - set(right))
    lines = dict.fromkeys(line for line in left + right if line not in removed)
    return ''.join(line + '\n' for line in lines)


def path_list(base: str, ours: str, theirs: str) -> str:
    """Three-way merge by corpus path, independent of append order/comments.

    Preserve either side's additions and removals. Different changes to the
    same path's expectation (including delete versus edit) stay conflicts.
    Sort entries just as ratchet writers do; standalone comments survive too.
    """
    def parse(text):
        entries, comments = {}, []
        for line in text.splitlines():
            if not line.strip() or line.startswith('#'):
                if line not in comments:
                    comments.append(line)
                continue
            path = line.split(' # ', 1)[0]
            if (not path.endswith('.hir') or any(c.isspace() for c in path)
                    or path.startswith('/') or '..' in path.split('/') or path in entries):
                raise Conflict('invalid or duplicate corpus path')
            entries[path] = line
        return entries, comments

    parsed = [parse(text) for text in (base, ours, theirs)]
    ancestor, left, right = [entries for entries, _ in parsed]
    lines = []
    for path in sorted(ancestor.keys() | left.keys() | right.keys()):
        old, a, b = (entries.get(path) for entries in (ancestor, left, right))
        if a == b:
            value = a
        elif a == old:
            value = b
        elif b == old:
            value = a
        else:
            raise Conflict('both sides changed one corpus expectation')
        if value is not None:
            lines.append(value)
    old_comments, a_comments, b_comments = [comments for _, comments in parsed]
    removed = (set(old_comments) - set(a_comments)) | (set(old_comments) - set(b_comments))
    comments = list(dict.fromkeys(line for line in a_comments + b_comments if line not in removed))
    return ''.join(line + '\n' for line in comments + lines)


def line_merge(base: str, ours: str, theirs: str, stretch) -> str:
    """A three-way line merge whose conflicting hunks `stretch` resolves from
    main's lines, the base's and the change's, and the lines around the hunk."""
    with tempfile.TemporaryDirectory() as scratch:
        names = []
        for name, text in (('ours', ours), ('base', base), ('theirs', theirs)):
            path = os.path.join(scratch, name)
            with open(path, 'w', encoding='utf-8', errors='surrogateescape') as file:
                file.write(text)
            names.append(path)
        merge = subprocess.run(['git', 'merge-file', '-p', '--diff3', '-L', 'ours', '-L', 'base', '-L', 'theirs',
                                *names], capture_output=True, encoding='utf-8', errors='surrogateescape')
    if merge.returncode < 0 or merge.returncode > 127:
        raise Conflict(merge.stderr)
    # Clean runs of lines (lists) and conflicting hunks (dicts), in order.
    parts, part, sides = [], None, {'ours': [], 'base': [], 'theirs': []}
    for line in merge.stdout.splitlines(keepends=True):
        if line.startswith('<<<<<<< ours'):
            part = 'ours'
        elif part == 'ours' and line.startswith('||||||| base'):
            part = 'base'
        elif part == 'base' and line.startswith('======='):
            part = 'theirs'
        elif part == 'theirs' and line.startswith('>>>>>>> theirs'):
            parts.append(sides)
            part, sides = None, {'ours': [], 'base': [], 'theirs': []}
        elif part is None:
            if not parts or isinstance(parts[-1], dict):
                parts.append([])
            parts[-1].append(line)
        else:
            sides[part].append(line)
    if part is not None:
        raise Conflict('unterminated conflict')
    out = []
    for index, chunk in enumerate(parts):
        if isinstance(chunk, list):
            out += chunk
            continue
        following = parts[index + 1] if index + 1 < len(parts) else None
        after = following[0] if isinstance(following, list) else None
        out += stretch(chunk['ours'], chunk['base'], chunk['theirs'], out[-1] if out else None, after)
    return ''.join(out)


def appended(ours, base, theirs, before, after):
    if base:
        raise Conflict('both sides edit the same lines')
    return ours + theirs


def key(line: str) -> str:
    return line.rstrip('\n').split('\t', 1)[0].split(' | ', 1)[0]


def sorted_stretch(ours, base, theirs, before, after):
    """Both sides' versions of one stretch of a sorted list, as one: what either
    side added, without what either side removed, in key order between the
    lines around it."""
    for side in (ours, base, theirs):
        if any(not line.endswith('\n') or not line.strip() or line.startswith('#') for line in side):
            raise Conflict('not a stretch of entries')
        if [key(line) for line in side] != sorted(key(line) for line in side):
            raise Conflict('a side is out of key order')
    removed = (set(base) - set(ours)) | (set(base) - set(theirs))
    lines = sorted({line for line in ours + theirs if line not in removed}, key=key)
    keys = [key(line) for line in lines]
    if len(set(keys)) != len(keys):
        raise Conflict('both sides changed one entry')
    if lines and ((before is not None and key(before) >= keys[0]) or (after is not None and keys[-1] >= key(after))):
        raise Conflict('the stretch is out of order with the lines around it')
    return lines


def manifest(base: str | None, ours: str, theirs: str) -> str:
    """A structural three-way merge of a package manifest, written as the
    repository writes them (two-space JSON); a side that is not in that form
    stays a conflict rather than being reformatted."""
    values = []
    for text in (ours, theirs):
        try:
            value = json.loads(text)
        except ValueError as error:
            raise Conflict(str(error)) from error
        if canonical(value) != text:
            raise Conflict('not in canonical form')
        values.append(value)
    try:
        ancestor = json.loads(base) if base else None
    except ValueError as error:
        raise Conflict(str(error)) from error
    return canonical(merge(ancestor, *values))


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


def install(root) -> None:
    """Make this file the clone's `hirsel` merge driver (.gitattributes)."""
    for name, value in (('name', 'files many lanes add to (tools/buck2/merging.py)'),
                        ('driver', 'python3 tools/buck2/merging.py %O %A %B %P')):
        subprocess.run(['git', 'config', f'merge.{DRIVER}.{name}', value], cwd=root, check=True)


def main(base_file: str, ours_file: str, theirs_file: str, path: str) -> int:
    """Git's merge driver: the merge into `ours_file`, or git's conflict
    markers there and exit status 1 when it stays a conflict."""
    def read(name):
        with open(name, encoding='utf-8', errors='surrogateescape') as file:
            return file.read()
    text = resolve(path, read(base_file), read(ours_file), read(theirs_file))
    if text is None:
        return 1 if subprocess.run(['git', 'merge-file', '-L', 'ours', '-L', 'base', '-L', 'theirs',
                                    ours_file, base_file, theirs_file]).returncode else 0
    with open(ours_file, 'w', encoding='utf-8', errors='surrogateescape') as file:
        file.write(text)
    return 0


if __name__ == '__main__':
    sys.exit(main(*sys.argv[1:]))
