#!/usr/bin/env python3
"""Per-case test costs from Buck2 test output, the input of cost-balanced shards.

Run every shard (or the one target) of the binaries to measure with libtest's timing report:

    .buck2/bin/buck2 test //:<binary>-shard-1 ... //:<binary>-shard-<n> -- \\
      --env RUSTC_BOOTSTRAP=1 --test-arg=-Zunstable-options --test-arg=--report-time \\
      --timeout 3600 > /tmp/costs.log 2>&1
    python3 tools/buck2/test_costs.py /tmp/costs.log
    python3 tools/buck2/sync.py

Each binary in sync.py's SHARDED list found in the log gets `test-costs/<binary>.tsv` (`case<TAB>seconds`,
sorted by case): the measured rows alone when the log has every shard of the
binary, so deleted cases leave the table, otherwise merged over its earlier rows. sync.py derives the binary's shard
count from the table's total and passes it to test_launcher.sh, which assigns
cases longest first. The report lists each target's wall time and the slowest
cases.
"""
import argparse
import json
from pathlib import Path
import re
import statistics

import event_logs

from sync import SHARDED

HERE = Path(__file__).resolve().parent
NATIVE_STAGES = {'gate-native-changed', 'gate-native-cranelift-changed'}
TABLES = HERE / 'test-costs'
ANSI = re.compile(r'\x1b\[[0-9;]*m')
TIMESTAMP = re.compile(r'^\[[^\]]+\]\s*')
TARGET = re.compile(r'^\S+ (Pass|Fail|Timeout|Fatal): root//:(\S+) \(([^)]*)\)')
CASE = event_logs.CASE_COST


def inventory(lines):
    """Libtest --list output grouped by Buck2's target headers."""
    cases, binary = {}, None
    for line in lines:
        line = TIMESTAMP.sub('', ANSI.sub('', line)).rstrip()
        if match := TARGET.match(line):
            binary = re.sub(r'-shard-\d+$', '', match[2])
            binary = binary if binary in SHARDED or binary in NATIVE_STAGES else None
        elif binary and (match := re.fullmatch(r'(\S+): test', line)):
            cases.setdefault(binary, set()).add(match[1])
    return cases


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
            binary = binary if binary in SHARDED or binary in NATIVE_STAGES else None
        elif binary in NATIVE_STAGES and (match := re.fullmatch(r'native case\s+(\S+)\s+([0-9.]+)\s+(?:[0-9]+|unknown)', line)):
            costs.setdefault(binary, {})[match[1]] = float(match[2])
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


def configured_shards(binary):
    """The graph that produced the log, before re-sizing from its new costs."""
    buck = (HERE.parents[1] / 'BUCK').read_text()
    block = re.search(r'hirsel_test\(\n    name = "' + re.escape(binary) + r'",(.*?)\n\)', buck, re.S)
    if not block:
        raise ValueError(f'no generated test target: {binary}')
    count = re.search(r'\n    shards = (\d+),', block[1])
    return int(count[1]) if count else 1


def write_table(path, rows):
    path.parent.mkdir(exist_ok=True)
    path.write_text(''.join(f'{case}\t{cost:.3f}\n' for case, cost in sorted(rows.items())))


def history_report(path, binary, since, min_gates):
    """Descriptive gate durations, not a cross-session performance verdict."""
    samples = {}
    for row in event_logs.test_history_rows(path):
        if row['target'] == binary and row['date'] >= since:
            samples.setdefault(row['shard'], {})[row['gate']] = float(row['duration_seconds'])
    if not samples:
        raise SystemExit(f'no timing history for {binary} since {since or "the beginning"}')
    print('shard\tgates\tmedian_seconds\tmax_seconds')
    for shard, gates in sorted(samples.items(), key=lambda row: int(row[0] or 0)):
        values = list(gates.values())
        print(f'{shard or "whole"}\t{len(gates)}\t{statistics.median(values):.1f}\t{max(values):.1f}')
    if any(len(gates) < min_gates for gates in samples.values()):
        raise SystemExit(f'not enough history: need at least {min_gates} distinct gates per shard')


def history_costs(path, binary, since):
    """Latest measured cases, including successful cases of incomplete gates."""
    rows, latest = {}, {}
    for row in event_logs.test_history_rows(path):
        target, date = row['target'], row['date']
        if target not in set(SHARDED) | NATIVE_STAGES or (binary != 'all' and target != binary) or date < since:
            continue
        for case, cost in json.loads(row.get('case_costs') or '{}').items():
            key = target, case
            if date >= latest.get(key, ''):
                latest[key] = date
                rows.setdefault(target, {})[case] = cost
    return rows


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('logs', nargs='*', type=Path)
    parser.add_argument('--report-only', action='store_true', help='print the report, write no table')
    parser.add_argument('--slowest', type=int, default=15)
    parser.add_argument('--history', nargs='?', const='', help='report the durable TSV (default: configured sink)')
    parser.add_argument('--from-history', nargs='?', const='', help='merge per-case timings from the durable TSV')
    parser.add_argument('--binary', default='hirsel-driver-test-it', help='binary to report or refresh (all for --from-history)')
    parser.add_argument('--since', default='', help='inclusive ISO-8601 UTC date/time, for example 2026-10-19')
    parser.add_argument('--min-gates', type=int, default=5)
    args = parser.parse_args()
    if args.history is not None or args.from_history is not None:
        if args.logs:
            parser.error('history options cannot be combined with cost logs')
        if args.history is not None and args.from_history is not None:
            parser.error('--history and --from-history are mutually exclusive')
        chosen = args.history if args.history is not None else args.from_history
        sink = event_logs.TsvTestHistory(chosen) if chosen else event_logs.test_history()
        if not isinstance(sink, event_logs.TsvTestHistory):
            parser.error('test history is disabled; pass an explicit TSV path')
        if args.history is not None:
            history_report(sink.path, args.binary, args.since, args.min_gates)
            return
        costs = history_costs(sink.path, args.binary, args.since)
        if not costs:
            parser.error('no per-case costs in the selected history; wait for real gate results')
        for binary, rows in sorted(costs.items()):
            print(f'{binary}: {len(rows)} measured cases from gate history')
            if not args.report_only:
                path = TABLES / f'{binary}.tsv'
                write_table(path, read_table(path) | rows)
        return
    if not args.logs:
        parser.error('provide cost logs, --history or --from-history')
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
            count = configured_shards(binary) if binary not in NATIVE_STAGES else 0
            complete = binary not in NATIVE_STAGES and (
                all(walls.get(f'{binary}-shard-{i}', ('',))[0] == 'Pass' for i in range(1, count + 1))
                if count > 1 else walls.get(binary, ('',))[0] == 'Pass')
            write_table(path, rows if complete else read_table(path) | rows)


if __name__ == '__main__':
    main()
