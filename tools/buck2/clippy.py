#!/usr/bin/env python3
"""Clippy every first-party Rust target, as `cargo clippy --workspace --all-targets`.

Builds each target's Clippy twin's `[clippy.txt]` subtarget on the pool (the
twins exist under `-c hirsel.clippy=true`, tools/buck2/defs.bzl) and prints what
Clippy reported. The subtarget records diagnostics without failing, so an error-level
diagnostic fails the stage here; warnings are printed and pass, as under Cargo.
"""

import argparse
import json
import os
from pathlib import Path
import re
import subprocess

import event_logs
import sys

ANSI = re.compile(r'\x1b\[[0-9;]*m')
ERROR = re.compile(r'^error(\[[A-Za-z0-9_:-]+\])?:', re.MULTILINE)

ROOT = Path(__file__).resolve().parents[2]
BUCK2 = ROOT / '.buck2/bin/buck2'
# The configuration tools/land and tools/main-runs run Buck2 with, plus the Clippy twins.
CONFIG = os.environ.get('HIRSEL_BUCK2_CONFIG', '').split() + ['-c', 'hirsel.clippy=true']


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--scope', help='Buck2 target query proven sufficient by tools/land')
    args = parser.parse_args()
    query = 'attrregexfilter(name, "-clippy$", kind("^rust_(library|binary)$", root//:))'
    if args.scope:
        query = f'({query}) intersect ({args.scope})'
    targets = subprocess.run(
        [str(BUCK2), 'uquery', *CONFIG, query],
        cwd=ROOT, check=True, capture_output=True, text=True).stdout.split()
    if not targets:
        raise SystemExit('clippy: empty target query; refusing an unchecked aggregate')
    report = event_logs.run(
        ['build', *CONFIG, '--show-full-json-output', *[t + '[clippy.txt]' for t in targets]],
        cwd=ROOT, capture_output=True, text=True)
    sys.stderr.write(report.stderr if report.returncode else '')
    if report.returncode:
        return report.returncode
    outputs = json.loads(report.stdout or '{}')
    if set(outputs) != {t + '[clippy.txt]' for t in targets}:
        raise SystemExit('clippy: incomplete diagnostics; refusing an unchecked aggregate')
    failed, text = diagnostics(outputs)
    print(text, end='')
    if failed:
        print('clippy: errors in ' + ', '.join(failed), file=sys.stderr)
        return 1
    return 0


def diagnostics(outputs):
    """The targets with an error-level diagnostic, and every report's text, from
    `buck2 build --show-full-json-output` of `[clippy.txt]` subtargets."""
    failed = []
    text = ''
    for target, path in sorted(outputs.items()):
        report = Path(path).read_text()
        if report.strip():
            text += report if report.endswith('\n') else report + '\n'
        if ERROR.search(ANSI.sub('', report)):
            failed.append(target.removeprefix('root//:').removesuffix('[clippy.txt]').removesuffix('-clippy'))
    return failed, text


if __name__ == '__main__':
    sys.exit(main())
