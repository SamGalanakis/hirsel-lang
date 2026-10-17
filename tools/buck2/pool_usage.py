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
"""

import argparse
from collections import defaultdict
import json
from pathlib import Path
import re
import subprocess

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


def names(lines):
    """Each operation's name from the workers' rows: its test binary, or its category."""
    found = {}
    for line in lines:
        fields = line.split('\t')
        row = dict(field.split('=', 1) for field in fields[1:] if '=' in field)
        op = row.get('op')
        if not op or op == '-':
            continue
        test = re.fullmatch(r'run:[^:]*:root//:(.*)', row.get('test', ''))
        if test:
            found[op] = re.sub(r'-shard-\d+$', '', test.group(1))
        elif op not in found:
            category = row.get('category', '-')
            crate = row.get('crate', '-')
            found[op] = f'{category} {crate}' if crate != '-' else f'{category if category != "-" else "other"} actions'
    return found


def field(identity, key):
    for part in identity.split('|'):
        if part.startswith(key + ':'):
            return part[len(key) + 1:]
    return '-'


def report(events, operation_names, repo, hours, top):
    newest = max(event['end_s'] for event in events)
    window = [event for event in events if event['end_s'] >= newest - hours * 3600]
    by_repo = defaultdict(float)
    lanes = defaultdict(lambda: [0, 0.0, 0.0, 0.0, 0.0])
    targets = defaultdict(lambda: [0, 0.0, 0.0, 0.0, 0.0])
    for event in window:
        wall_h = event['wall_time_ms'] / HOUR_MS
        reserved_cpu = event['effective_cpu'] * wall_h
        by_repo[field(event['identity'], 'repo')] += reserved_cpu
        if field(event['identity'], 'repo') != repo:
            continue
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
    out += table('lane', lanes) + table('target', targets)
    return '\n'.join(out)


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('--hours', type=float, default=6)
    parser.add_argument('--repo', default='hirsel-lang', help='the identity\'s repo: tag')
    parser.add_argument('--top', type=int, default=40, help='rows per table')
    parser.add_argument('--trace', nargs='*', default=[], help='copies of the scheduler\'s usage-event trace files')
    parser.add_argument('--usage', nargs='*', default=[], help='copies of the workers\' actions.log')
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
    print(report(events, names(usage), args.repo, args.hours, args.top))


if __name__ == '__main__':
    main()
