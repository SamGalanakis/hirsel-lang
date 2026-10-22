#!/usr/bin/env python3
"""rustfmt every first-party crate with the pinned toolchain, as `cargo fmt`.

`--check` reports files that are not formatted and fails, as `cargo fmt --check`.
rustfmt starts from each target's root file and follows its modules.
"""

import argparse
import json
import os
from pathlib import Path
import re
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[2]
BUCK2 = ROOT / '.buck2/bin/buck2'
# The configuration tools/land and tools/main-runs run Buck2 with, so the daemon keeps one state.
CONFIG = os.environ.get('HIRSEL_BUCK2_CONFIG', '').split()
RUSTFMT = ROOT / 'tools/buck2/toolchains/rust-files/bin/rustfmt'


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--scope', help='Buck2 target query proven sufficient by tools/land')
    args, rustfmt_flags = parser.parse_known_args()
    query = 'kind("^rust_(library|binary)$", root//:)'
    if args.scope:
        query = f'({query}) intersect ({args.scope})'
    attributes = json.loads(subprocess.run(
        [str(BUCK2), 'uquery', *CONFIG, query,
         '--output-attribute', 'crate_root', '--output-attribute', 'edition'],
        cwd=ROOT, check=True, capture_output=True, text=True).stdout)
    if not attributes:
        raise SystemExit('fmt: empty target query; refusing an unchecked aggregate')
    roots = {}
    for target in attributes.values():
        roots.setdefault(target['edition'], set()).add(target['crate_root'])
        # rustfmt does not follow include!. Check each declared module as a
        # root, with its harness's edition. The pure declaration list's syntax
        # is checked by check-hot-files; its union-merged order is immaterial.
        registry = Path(target['crate_root']).with_name('modules.rs')
        if (ROOT / registry).is_file() and registry.parts[:1] == ('crates',):
            for line in (ROOT / registry).read_text().splitlines():
                if match := re.fullmatch(r'mod (\w+);', line):
                    source = registry.with_name(match[1] + '.rs')
                    if not (ROOT / source).is_file():
                        source = registry.parent / match[1] / 'mod.rs'
                    roots[target['edition']].add(str(source))
    status = 0
    for edition, files in sorted(roots.items()):
        command = [str(RUSTFMT), '--edition', edition, *rustfmt_flags, *sorted(files)]
        status = subprocess.run(command, cwd=ROOT).returncode or status
    return status


if __name__ == '__main__':
    sys.exit(main())
