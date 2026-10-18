#!/usr/bin/env python3
"""What hirsel reserves on the pool against what it uses, read-only.

    python3 tools/buck2/pool_usage.py --hours 6 --trace /tmp/trace-*.jsonl --usage /tmp/usage-*.log
    python3 tools/buck2/pool_usage.py --hours 6 --ssh-cache <cache host> --ssh-workers <worker> ...

The scheduler's usage events (kiln-resource-hints `trace/*.jsonl` on the cache
host) give each execution its client identity (`repo:<repo>|lane:...`), the
CPUs and memory it was placed with, its CPU time, wall time and PSS peak. The
workers' `actions.log` rows name an execution's test target by its operation
(`test=run:<shard>:root//:<target>`); other actions are named by their
category. Over the last N hours before the newest event, an execution reserves
CPUs x wall and memory x wall, and uses its CPU time and PSS peak x wall (an
upper bound). The report prints the pool's reserved CPU-hours by repository,
the repository's share of them, and its reserved and used CPU-hours and
GiB-hours per lane and per target, a test target's shards as one row.
Compressed gate/land logs are retained across worktrees (event_logs.py) and
automatically joined for target attribution and repeated action-key evidence.
It also sums execution wall seconds and CPU seconds per action category;
queueing is excluded. With --events (exported Buck2 JSONL logs), uniquely
matched rustc actions are split into compile and link. Rustc's link action
includes compiling the executable; it is not a linker-only measurement.
"""

import argparse
from collections import defaultdict
import json
from pathlib import Path
import re
import subprocess

from action_sizes_from_log import join
import event_logs

TRACE_DIR = '/var/lib/kiln-resource-hints/trace'
USAGE_LOGS = ('/workspace/kiln-executor/usage/actions.log.1', '/workspace/kiln-executor/usage/actions.log')
HOUR_MS = 3_600_000
GIB_KB = 1 << 20


def ssh_cat(host, command):
    return subprocess.run(['ssh', host, command], capture_output=True, text=True, check=True).stdout


def executions(lines):
    """The scheduler's usage events, one per execution attempt."""
    seen = {}
    for line in lines:
        if '"usage"' not in line:
            continue
        event = json.loads(line)
        if event.get('event') == 'usage' and event.get('wall_time_ms') is not None:
            seen[(event['operation'], event.get('attempt'))] = event
    return list(seen.values())


def operations(lines, wanted):
    """Worker measurements by operation, including their finish timestamps."""
    found = {}
    for line in lines:
        fields = line.rstrip('\n').split('\t')
        if not fields[0].isdigit():
            continue
        row = dict(field.split('=', 1) for field in fields[1:] if '=' in field)
        op = row.get('op')
        if op in wanted:
            previous = found.get(op, {})
            current = {key: value for key, value in row.items() if value != '-'} | {'end': int(fields[0])}
            # Retries may move between workers. File/host order is not time
            # order: an older killed attempt must not erase the retry's exit.
            found[op] = (previous | current if current['end'] >= previous.get('end', 0)
                         else current | previous)
    return found


def names(rows):
    """Each operation's name: its test binary, or its category and crate."""
    found = {}
    for op, row in rows.items():
        test = re.fullmatch(r'run:[^:]*:root//:(.*)', row.get('test', ''))
        if test:
            found[op] = re.sub(r'-shard-\d+$', '', test.group(1))
        else:
            category = row.get('category', '-')
            crate = row.get('crate', '-')
            found[op] = f'{category} {crate}' if crate != '-' else f'{category if category != "-" else "other"} actions'
    return found


def categories(rows):
    found = {}
    for op, row in rows.items():
        category = row.get('category', '-')
        if row.get('test', '-').startswith('run:'):
            category = 'test'
        elif category == 'rustc':
            category = 'rustc (phase unknown)'
        found[op] = category if category != '-' else 'other'
    return found


def attribute_actions(commands, rows, operation_names, operation_categories):
    """Use the sizing tool's conservative join for named compile phases."""
    records = {op: row for op, row in rows.items()
               if row.get('category') == 'rustc' and row.get('exit') == '0'
               and all(row.get(key, '-').isdigit() for key in
                       ('wall_ms', 'cpu_usec', 'peak_bytes', 'requested_cpu', 'requested_kb'))}
    actions = [c for c in commands if c['category'] == 'rustc' and not c['cached'] and not c['failed']]
    for action, row in join(actions, records):
        if row is None:
            continue
        op = row['op']
        operation_names[op] = f"rustc {action['profile']} {action['display']} {action['phase']}"
        operation_categories[op] = 'rustc link' if action['phase'].split()[0] == 'link' else 'rustc compile'


def repeated_actions(commands, newest, hours, top):
    """Repeated real executions, excluding hits and merged/copy event records."""
    groups = defaultdict(list)
    for command in commands:
        if not command['cached'] and newest - hours * 3600 <= command['end'] <= newest:
            groups[(command['display'], command['digest'])].append(command)
    repeated = {key: runs for key, runs in groups.items() if len(runs) > 1}
    lines = [f'repeated action keys: {len(repeated)} (retained logs only; failed attempts are not cached)']
    for (target, digest), runs in sorted(repeated.items(), key=lambda row: -len(row[1]))[:top]:
        successes = sum(not run['failed'] for run in runs)
        lines.append(f'  {target}: {len(runs)} executions, {successes} successful; {digest}')
    return '\n'.join(lines)


def field(identity, key):
    for part in identity.split('|'):
        if part.startswith(key + ':'):
            return part[len(key) + 1:]
    return '-'


def report(events, operation_names, operation_categories, repo, hours, top):
    newest = max(event['end_s'] for event in events)
    window = [event for event in events if event['end_s'] >= newest - hours * 3600]
    by_repo = defaultdict(float)
    lanes = defaultdict(lambda: [0, 0.0, 0.0, 0.0, 0.0])
    targets = defaultdict(lambda: [0, 0.0, 0.0, 0.0, 0.0])
    action_categories = defaultdict(lambda: [0, 0.0, 0.0])
    for event in window:
        wall_h = event['wall_time_ms'] / HOUR_MS
        reserved_cpu = event['effective_cpu'] * wall_h
        by_repo[field(event['identity'], 'repo')] += reserved_cpu
        if field(event['identity'], 'repo') != repo:
            continue
        category = operation_categories.get(event['operation'], 'unknown (no worker row)')
        execution = (1, event['wall_time_ms'] / 1000, event.get('cpu_time_ms', 0) / 1000)
        action_categories[category] = [a + b for a, b in zip(action_categories[category], execution)]
        row = (1, reserved_cpu, event.get('cpu_time_ms', 0) / HOUR_MS,
               event['effective_memory_kb'] / GIB_KB * wall_h, event.get('peak_memory_kb', 0) / GIB_KB * wall_h)
        for table, key in ((lanes, field(event['identity'], 'lane')),
                           (targets, operation_names.get(event['operation'], 'unnamed (no worker row)'))):
            table[key] = [a + b for a, b in zip(table[key], row)]
    pool = sum(by_repo.values())
    out = [f'pool: {pool:.1f} reserved CPU-hours in the {hours} h before the newest event ({len(window)} executions)']
    out += [f'  {name:24s} {cpu:8.1f} CPU-h  {cpu / pool:6.1%}' for name, cpu in sorted(by_repo.items(), key=lambda x: -x[1])]
    total = [sum(row[i] for row in lanes.values()) for i in range(5)]

    def table(title, rows):
        lines = [f'{title:44s} {"runs":>6s} {"res CPU-h":>10s} {"used":>8s} {"used%":>6s} {"res GiB-h":>10s} {"used":>8s}']
        for name, (n, cpu, used, gib, used_gib) in sorted(rows.items(), key=lambda x: -x[1][1])[:top]:
            lines.append(f'{name[:44]:44s} {n:6d} {cpu:10.2f} {used:8.2f} {used / cpu if cpu else 0:6.0%} {gib:10.2f} {used_gib:8.2f}')
        return lines

    out.append(f'{repo}: {total[1]:.1f} reserved CPU-hours, {total[1] / pool if pool else 0:.1%} of the pool\'s; '
               f'used {total[2]:.1f} ({total[2] / total[1] if total[1] else 0:.0%}); '
               f'{total[3]:.1f} reserved GiB-hours, at most {total[4]:.1f} used')
    out.append(f'{"action category":44s} {"runs":>6s} {"exec wall s":>12s} {"CPU s":>12s}')
    for name, (n, wall, cpu) in sorted(action_categories.items(), key=lambda x: -x[1][1])[:top]:
        out.append(f'{name:44s} {n:6d} {wall:12.2f} {cpu:12.2f}')
    out += table('lane', lanes) + table('target', targets)
    return '\n'.join(out)


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('--hours', type=float, default=6)
    parser.add_argument('--repo', default='hirsel-lang', help='the identity\'s repo: tag')
    parser.add_argument('--top', type=int, default=40, help='rows per table')
    parser.add_argument('--trace', nargs='*', default=[], help='copies of the scheduler\'s usage-event trace files')
    parser.add_argument('--usage', nargs='*', default=[], help='copies of the workers\' actions.log')
    parser.add_argument('--events', nargs='*', default=[], help='Buck2 JSONL or compressed .pb.zst logs')
    parser.add_argument('--event-dir', type=Path, default=event_logs.directory(),
                        help='shared retained logs (default: HIRSEL_BUCK2_LOG_DIR or ~/.cache/hirsel-buck2/event-logs)')
    parser.add_argument('--ssh-cache', help=f'read {TRACE_DIR} on this host with ssh and sudo -n')
    parser.add_argument('--ssh-workers', nargs='*', default=[], help='read the usage logs on these hosts')
    args = parser.parse_args()
    trace = [line for path in args.trace for line in Path(path).read_text().splitlines()]
    if args.ssh_cache:
        trace += ssh_cat(args.ssh_cache, f'cd {TRACE_DIR} && sudo -n cat *.jsonl').splitlines()
    usage = [line for path in args.usage for line in Path(path).read_text().splitlines()]
    for host in args.ssh_workers:
        usage += ssh_cat(host, 'sudo -n cat ' + ' '.join(USAGE_LOGS)).splitlines()
    events = executions(trace)
    if not events:
        parser.error('no usage events: pass --trace or --ssh-cache')
    rows = operations(usage, {event['operation'] for event in events if field(event['identity'], 'repo') == args.repo})
    operation_names, operation_categories = names(rows), categories(rows)
    paths = sorted({Path(p).resolve() for p in args.events} |
                   set(event_logs.discover(args.event_dir, args.hours)))
    commands = list(event_logs.commands(paths))
    attribute_actions(commands, rows, operation_names, operation_categories)
    print(report(events, operation_names, operation_categories, args.repo, args.hours, args.top))
    print(f'event-log coverage: {len(paths)} files, {len(commands)} distinct remote command records')
    print(repeated_actions(commands, max(e['end_s'] for e in events), args.hours, args.top))


if __name__ == '__main__':
    main()
