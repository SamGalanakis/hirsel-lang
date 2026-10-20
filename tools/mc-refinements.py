#!/usr/bin/env python3
"""M0b fixed-queue cost model; not an executor or a semantic oracle.

Rows are turns in a known serial reference schedule. Children are already
spawned. `possible` includes captured handles, type closure, holding children
and filtered polls. No footprint inference from M0's timing-only log is sound.
E may complete early only in the first queued turn, never a later speculative
turn. Pending sends are published at retirement, not at early completion.
"""
import heapq


def conflicts(a, b):
    return bool(set(a.get('write', ())) & set(b.get('read', ()) + b.get('write', ()))
                or set(a.get('read', ())) & set(b.get('write', ())))


def simulate(rows, width, mode='strict', handoff_ns=0, commit_ns=200, log_ns=20, run_ahead=32):
    if width < 1 or mode not in ('strict', 'ahead', 'early') or min(handoff_ns, commit_ns, log_ns, run_ahead) < 0:
        raise ValueError('invalid model parameters')
    n = len(rows)
    previous, last = [], {}
    for i, row in enumerate(rows):
        if row['kind'] not in ('D', 'E', 'H') or row['ns'] < 0:
            raise ValueError('invalid turn')
        previous.append(last.get(row['task'], -1))
        last[row['task']] = i
    state = ['new'] * n
    action_done = [False] * n
    retired = [False] * n
    retire_time = [-1] * n
    running = []
    role = None
    now = head_start = head = 0
    early = logged = max_pending = 0
    publish = []
    deferred_publish = set()
    early_history = []
    stall = 0

    def may_early(i):
        # Rule 1: queued when this head started; previous turn retired then.
        prev = previous[i]
        if prev >= 0 and (not retired[prev] or retire_time[prev] > head_start):
            return False
        # Rule 2: the model has one terminal op per turn, no uncommitted
        # earlier op. Only explicitly certified object-only E rows qualify.
        if rows[i]['kind'] != 'E':
            return False
        # Rule 3: include *every* unresolved predecessor's possible footprint.
        return all(retired[j] or not conflicts(rows[i], rows[j]['possible'])
                   for j in range(head, i))

    while head < n:
        while running and running[0][0] == now:
            _, i = heapq.heappop(running)
            state[i] = 'pending'
        if role is not None and role[0] == now:
            _, i, retire = role
            role = None
            action_done[i] = True
            if retire:
                retired[i] = True
                retire_time[i] = now
                if rows[i].get('send'):
                    publish.append((i, now))
                    deferred_publish.discard(i)
                head += 1
                head_start = now
            else:
                early += 1
                early_history.append((head, i))
                if rows[i].get('send'):
                    deferred_publish.add(i)
                    max_pending = max(max_pending, len(deferred_publish))
        if head == n:
            break
        # Refinement 1: D ops log, but their effects wait for retirement.
        for i in range(head+1, n):
            if (mode != 'strict' and state[i] == 'pending' and rows[i]['kind'] == 'D'
                    and not action_done[i] and sum(action_done[j] and not retired[j]
                        and rows[j]['task'] == rows[i]['task'] for j in range(head, i)) < run_ahead):
                action_done[i] = True
                logged += 1
        # Earliest ready segments occupy workers, including the head.
        for i in range(head, n):
            prev = previous[i]
            ready = prev < 0 or retired[prev] or (mode != 'strict' and action_done[prev])
            if state[i] == 'new' and ready and len(running) < width:
                state[i] = 'running'
                # Each dispatch pays one measured role handoff; logging is local.
                cost = rows[i]['ns'] + handoff_ns
                if mode != 'strict' and prev >= 0 and not retired[prev] and rows[prev]['kind'] == 'D':
                    cost += log_ns
                heapq.heappush(running, (now + cost, i))
        if role is None:
            if state[head] == 'pending':
                role = (now + commit_ns, head, True)
            elif mode == 'early':
                for i in range(head+1, n):
                    if state[i] == 'pending' and not action_done[i] and may_early(i):
                        role = (now + commit_ns, i, False)
                        break
        events = ([running[0][0]] if running else []) + ([role[0]] if role else [])
        if head == n:
            break
        if not events:
            raise ValueError('model made no progress')
        end = min(events)
        if state[head] != 'pending':
            stall += end-now
        now = end
    return dict(makespan_ns=now, early_commits=early, logged=logged,
                max_pending=max_pending, publish=publish, early_history=early_history,
                hol_share=stall/now if now else 0)


def scenario(kind='E', segment_ns=100000, tasks=8, rounds=4, shared=False):
    """One long head and prequeued siblings, then their continuation turns.

    The head holds either its own object or every sibling's object. Each
    sibling's first op enables a substantial next segment; repeated short ops
    behind that segment expose rule 1 rather than assuming unlimited bypass.
    """
    rows = []
    for turn in range(rounds):
        for task in range(tasks):
            obj = 'shared' if shared else str(task)
            read = [obj] if shared or task == 0 else []
            rows.append(dict(task=task, ns=(8 * segment_ns if turn == 0 else 0) if task == 0 else segment_ns,
                             kind='H' if task == 0 else kind,
                             read=[], write=[obj], send=kind == 'E' and task != 0,
                             possible=dict(read=read, write=[obj])))
    return rows
