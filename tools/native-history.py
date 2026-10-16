#!/usr/bin/env python3
"""Derive native finding pins from every recorded exclusion table in Git history."""
import argparse
from pathlib import Path
import subprocess

ROOT = Path(__file__).resolve().parents[1]
TABLE = 'native-tests/regressions/exclusions.tsv'
OUTPUT = ROOT / 'native-tests/regressions/history.tsv'


def derive():
    commits = subprocess.check_output(['git', 'log', '--all', '--format=%H', '--', TABLE], cwd=ROOT, text=True).splitlines()
    paths = {}
    for commit in reversed(commits):
        result = subprocess.run(['git', 'show', f'{commit}:{TABLE}'], cwd=ROOT, text=True, capture_output=True)
        if result.returncode:
            continue  # A deletion is itself a historical change to the table.
        for row in result.stdout.splitlines():
            if row and not row.startswith('#'):
                path = row.split('\t')[0]
                if path.startswith('conformance/') and path.endswith('.hir'):
                    paths.setdefault(path, commit)
    return '# Derived by tools/native-history.py; first recorded exclusion commit per source.\n' + ''.join(
        f'{path}\t{commit}\n' for path, commit in sorted(paths.items()))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--check', action='store_true')
    args = parser.parse_args()
    text = derive()
    if args.check:
        if not OUTPUT.is_file() or OUTPUT.read_text() != text:
            parser.exit(1, 'native history pins are stale; run python3 tools/native-history.py\n')
    else:
        OUTPUT.write_text(text)
        rows = text.splitlines()[1:]
        missing = [row.split('\t')[0] for row in rows if not ROOT.joinpath(row.split('\t')[0]).is_file()]
        print(f'{len(rows)} historical exclusion programs; {len(missing)} deleted source paths')
        for path in missing:
            print(f'deleted: {path}')


if __name__ == '__main__':
    main()
