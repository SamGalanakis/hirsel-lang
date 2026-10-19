#!/usr/bin/env python3
"""Replay cooperative segment measurements with parallel workers and ordered commits.

Input: the ordinary measurement report with `mc-profile 1` / `mc {JSON}` rows.
`after` is a one-based commit dependency (0 = entry); rows are commit order.
Workers take eligible segments in commit order, without preemption. A worker is
free when its segment finishes, even if that segment cannot commit yet. The
serial commit server can overlap workers, including at N=1. Host wait durations
are excluded: this is a compute potential model, conditional on recorded input
readiness. No queueing, migration, cache, or handoff overhead is modelled.
"""
import argparse
import heapq
import json
from pathlib import Path

WORKERS = (1, 2, 4, 8, 16)


def read_log(path):
    lines = Path(path).read_text().splitlines()
    if lines.count('mc-profile 1') != 1:
        raise ValueError('expected one mc-profile 1 trace')
    rows = [json.loads(line[3:]) for line in lines if line.startswith('mc ')]
    previous = {}
    for commit, row in enumerate(rows, 1):
        task = tuple(row['task'])
        if (any(type(n) is not int or n < 0 for n in task)
                or type(row['ns']) is not int or row['ns'] < 0
                or type(row['after']) is not int
                or not previous.get(task, 0) <= row['after'] < commit
                or not isinstance(row['operation'], str)):
            raise ValueError(f'invalid duration or dependency at commit {commit}')
        # The first segment of a child must follow its creation.
        if task and row['after'] == 0:
            raise ValueError(f'child available before spawn at commit {commit}')
        previous[task] = commit
    if not rows:
        raise ValueError('empty trace')
    return rows


def simulate(rows, workers, commit_ns=200):
    """Event-driven list schedule, with a separate, strictly ordered commit server."""
    if workers < 1 or commit_ns < 0:
        raise ValueError('workers must be positive and commit cost nonnegative')
    dependents = [[] for _ in range(len(rows) + 1)]
    for i, row in enumerate(rows):
        dependents[row['after']].append(i)
    ready = list(dependents[0])
    heapq.heapify(ready)
    running = []  # (finish time, row index)
    finished = set()
    now = 0
    next_commit = 0
    committing = None  # (end, row index)
    stall = 0
    hol = 0  # wait while a later segment is already finished
    while next_commit < len(rows):
        while ready and len(running) < workers:
            i = heapq.heappop(ready)
            heapq.heappush(running, (now + rows[i]['ns'], i))
        while running and running[0][0] == now:
            _, i = heapq.heappop(running)
            finished.add(i)
        if committing is not None and committing[0] == now:
            _, i = committing
            committing = None
            next_commit += 1
            for child in dependents[i + 1]:
                heapq.heappush(ready, child)
            continue
        if committing is None and next_commit in finished:
            finished.remove(next_commit)
            committing = (now + commit_ns, next_commit)
            continue
        # A just-finished segment frees its worker even while waiting to commit.
        if ready and len(running) < workers:
            continue
        events = ([running[0][0]] if running else []) + ([committing[0]] if committing else [])
        if not events:
            raise ValueError('trace has a dependency cycle')
        end = min(events)
        if committing is None:
            stall += end - now
            if finished:
                hol += end - now
        now = end
    return dict(workers=workers, makespan_ns=now,
                stall_share=stall / now if now else 0,
                hol_ready_share=hol / now if now else 0,
                commit_busy_share=len(rows) * commit_ns / now if now else 0)


def bounds(rows, commit_ns):
    # Longest path through segment->commit dependencies and serial commit edges.
    # This is the infinite-worker ordered span; no worker scheduling heuristic.
    commits = [0]
    work = sum(r['ns'] for r in rows) + len(rows) * commit_ns
    for row in rows:
        commits.append(max(commits[-1], commits[row['after']] + row['ns']) + commit_ns)
    return dict(work_ns=work, span_ns=commits[-1],
                work_span=work / commits[-1] if commits[-1] else 0)


def report(rows, commit_ns):
    results = [simulate(rows, n, commit_ns) for n in WORKERS]
    baseline = results[0]['makespan_ns']
    bound = bounds(rows, commit_ns)
    for result in results:
        result['speedup'] = baseline / result['makespan_ns'] if result['makespan_ns'] else 1
        # There are N segment workers plus a serial commit server.
        lower = max(bound['span_ns'], sum(r['ns'] for r in rows) / result['workers'],
                    bound['work_ns'] / (result['workers'] + 1))
        result['speedup_upper_bound'] = baseline / lower if lower else 1
    return dict(schema=1, segments=len(rows), tasks=len({tuple(r['task']) for r in rows}),
                commit_ns=commit_ns, bounds=bound, results=results)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('logs', type=Path, nargs='+')
    parser.add_argument('--commit-ns', type=int, default=200,
                        help='fixed cost per committed operation (default: 200 ns)')
    parser.add_argument('--json', action='store_true')
    args = parser.parse_args()
    if args.commit_ns < 0:
        parser.error('--commit-ns must be nonnegative')
    reports = {str(path): report(read_log(path), args.commit_ns) for path in args.logs}
    if args.json:
        print(json.dumps(reports, indent=2))
        return
    print('Fixed commit cost:', args.commit_ns, 'ns; N segment workers + serial commit server.')
    print('Stall = all commit-server wait; ready-HOL = wait with a later result ready.')
    for path, data in reports.items():
        print(f'\n{path}: {data["segments"]} segments, {data["tasks"]} tasks; '
              f'work/span={data["bounds"]["work_span"]:.3f}, '
              f'span={data["bounds"]["span_ns"]} ns')
        print('N  makespan_ns  speedup  stall%  ready-HOL%  busy%  upper')
        for r in data['results']:
            print(f'{r["workers"]:2} {r["makespan_ns"]:12} {r["speedup"]:7.3f} '
                  f'{100*r["stall_share"]:7.2f} {100*r["hol_ready_share"]:10.2f} '
                  f'{100*r["commit_busy_share"]:6.2f} {r["speedup_upper_bound"]:6.3f}')


if __name__ == '__main__':
    main()
