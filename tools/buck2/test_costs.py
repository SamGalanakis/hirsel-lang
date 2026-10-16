#!/usr/bin/env python3
"""Per-case test costs from Buck2 test output, the input of cost-balanced shards.

Run every shard (or the one target) of the binaries to measure with libtest's timing report:

    .buck2/bin/buck2 test //:<binary>-shard-1 ... //:<binary>-shard-<n> -- \\
      --env RUSTC_BOOTSTRAP=1 --test-arg=-Zunstable-options --test-arg=--report-time \\
      --timeout 3600 > /tmp/costs.log 2>&1
    python3 tools/buck2/test_costs.py /tmp/costs.log
    python3 tools/buck2/sync.py

Each binary in sync.py's SHARDED list found in the log gets `test-costs/<binary>.tsv` (`case<TAB>seconds`,
sorted by case), merged over its earlier rows. sync.py derives the binary's shard
count from the table's total and passes it to test_launcher.sh, which assigns
cases longest first. The report lists each target's wall time and the slowest
cases.
"""
import argparse
from pathlib import Path
import re

from sync import SHARDED

HERE = Path(__file__).resolve().parent
TABLES = HERE / 'test-costs'
ANSI = re.compile(r'\x1b\[[0-9;]*m')
TIMESTAMP = re.compile(r'^\[[^\]]+\]\s*')
TARGET = re.compile(r'^\S+ (Pass|Fail|Timeout|Fatal): root//:(\S+) \(([^)]*)\)')
CASE = re.compile(r'^test (\S+)(?: - should panic)? \.\.\. ok <(\d+(?:\.\d+)?)s>')


def seconds(text):
    """Buck2's durations: `36.3s`, `1:10.5s` or `1:02:03.0s`."""
    total = 0.0
    for part in text.rstrip('s').split(':'):
        total = total * 60 + float(part)
    return total


def parse(lines):
    """(target walls, {binary: {case: seconds}}) from Buck2's combined output."""
    walls, costs, binary = {}, {}, None
    for line in lines:
        line = TIMESTAMP.sub('', ANSI.sub('', line)).rstrip()
        if match := TARGET.match(line):
            status, target, wall = match.groups()
            walls[target] = (status, seconds(wall))
            binary = re.sub(r'-shard-\d+$', '', target)
            binary = binary if binary in SHARDED else None
        elif binary and (match := CASE.match(line)):
            costs.setdefault(binary, {})[match.group(1)] = float(match.group(2))
    return walls, costs


def read_table(path):
    rows = {}
    if path.exists():
        for line in path.read_text().splitlines():
            case, cost = line.split('\t')
            rows[case] = float(cost)
    return rows


def write_table(path, rows):
    path.parent.mkdir(exist_ok=True)
    path.write_text(''.join(f'{case}\t{cost:.1f}\n' for case, cost in sorted(rows.items())))


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('logs', nargs='+', type=Path)
    parser.add_argument('--report-only', action='store_true', help='print the report, write no table')
    parser.add_argument('--slowest', type=int, default=15)
    args = parser.parse_args()
    walls, costs = {}, {}
    for log in args.logs:
        with log.open(errors='replace') as lines:
            log_walls, log_costs = parse(lines)
        walls.update(log_walls)
        for binary, rows in log_costs.items():
            costs.setdefault(binary, {}).update(rows)
    for target, (status, wall) in sorted(walls.items()):
        print(f'{wall:8.1f} s  {status:7}  {target}')
    for binary, rows in sorted(costs.items()):
        print(f'\n{binary}: {len(rows)} cases, {sum(rows.values()):.1f} test-seconds')
        for case, cost in sorted(rows.items(), key=lambda row: -row[1])[:args.slowest]:
            print(f'{cost:8.1f} s  {case}')
        if not args.report_only:
            path = TABLES / f'{binary}.tsv'
            write_table(path, read_table(path) | rows)


if __name__ == '__main__':
    main()
