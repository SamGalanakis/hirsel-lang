#!/usr/bin/env python3
"""M0 timing DAG placement, with last-worker preference and oldest-first steal.

This estimates migration refresh frequency, not a live scheduler or throughput.
Known keep entries are folded into their surrounding turns before scheduling.
Recorded readiness and commit order are preserved. A task cannot execute two
segments simultaneously. Page changes are not traced, so epoch refreshes are
excluded; a migration is an unavoidable refresh in the inline-view design.
"""
import heapq


# Exact no-scheduling-point entries from the design; unknown entries split a turn.
KEEP = {'endpoint.close', 'mutex_take', 'mutex_cause', 'task_set_has',
        'task_set_take', 'task_scope_enter', 'task_scope_enter_until',
        'hirsel_rt_v2_arg_len', 'hirsel_rt_v2_arg_text'}


def turns(rows):
    result, group_of = [], [0]
    for i, row in enumerate(rows, 1):
        if (result and result[-1]['task'] == row['task']
                and result[-1]['operation'] in KEEP):
            group = result[-1]
            group['ns'] += row['ns']
            group['operation'] = row['operation']
            if group_of[row['after']] != len(result):
                group['after'] = max(group['after'], group_of[row['after']])
        else:
            result.append(dict(row, after=group_of[row['after']]))
        group_of.append(len(result))
    return result


def simulate(rows, width, commit_ns=200):
    if not rows or width < 1 or commit_ns < 0:
        raise ValueError('empty trace or invalid model parameters')
    ready, running, finished = set(), [], set()
    deps = [[] for _ in range(len(rows)+1)]
    for i, row in enumerate(rows):
        deps[row['after']].append(i)
    ready.update(deps[0])
    idle = set(range(width))
    last_worker, on_worker = {}, {}
    now = head = migrations = switches = revisits = worker_task_changes = 0
    role = None
    while head < len(rows):
        while running and running[0][0] == now:
            _, i, worker = heapq.heappop(running)
            finished.add(i)
            idle.add(worker)
        if role is not None and role[0] == now:
            _, i = role
            head += 1
            role = None
            ready.update(deps[i+1])
        while ready and idle:
            i = min(ready)
            task = tuple(rows[i]['task'])
            preferred = last_worker.get(task)
            worker = preferred if preferred in idle else min(idle)
            ready.remove(i)
            idle.remove(worker)
            # Each turn dispatch binds a view, even if this worker was idle
            # since the same task last ran here (notably channel ping-pong).
            switches += 1
            if on_worker.get(worker) != task:
                worker_task_changes += 1
            on_worker[worker] = task
            if task in last_worker:
                revisits += 1
                migrations += preferred != worker
            last_worker[task] = worker
            heapq.heappush(running, (now + rows[i]['ns'], i, worker))
        if role is None and head in finished:
            finished.remove(head)
            role = (now + commit_ns, head)
        events = ([running[0][0]] if running else []) + ([role[0]] if role else [])
        if head == len(rows):
            break
        if not events:
            raise ValueError('invalid DAG')
        now = min(events)
    return dict(width=width, segments=len(rows), switches=switches, worker_task_changes=worker_task_changes, revisits=revisits,
                migrations=migrations, migration_per_dispatch=migrations/len(rows),
                migration_per_switch=migrations/switches if switches else 0)
