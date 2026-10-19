"""Counted, pinned samples and session-local ratio analysis."""
import fcntl
import hashlib
import math
import os
from pathlib import Path
import random
import statistics
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[2]
EVENTS = ('task-clock', 'cycles', 'instructions', 'cycles:u', 'instructions:u')
TIMING_LOCK = Path('/workspace/code/hirsel-briefs/locks/cpu31-timing.lock')


def perf_counts(text, events=EVENTS):
    """Read perf's locale-independent CSV; missing/unsupported counters fail closed.

    perf scales multiplexed counts itself. Keep running percentages so a caller
    can inspect multiplexing rather than mistaking a partial count for a win.
    """
    counts, running = {}, {}
    for line in text.splitlines():
        fields = line.split(';')
        if len(fields) < 3 or fields[2] not in events:
            continue
        event = fields[2]
        try:
            value = float(fields[0])
            if not math.isfinite(value) or value <= 0:
                raise ValueError()
            counts[event] = value
            running[event] = float(fields[4]) if len(fields) > 4 and fields[4] else None
        except ValueError as error:
            raise ValueError(f'perf counter {event} unavailable: {line}') from error
    missing = set(events) - counts.keys()
    if missing:
        raise ValueError(f'perf counters missing: {sorted(missing)}')
    return counts, running


def cpu_list(text):
    cpus = []
    for item in text.strip().split(','):
        ends = [int(n) for n in item.split('-')]
        cpus.extend(range(ends[0], ends[-1] + 1))
    return cpus


def siblings(cpu):
    path = Path(f'/sys/devices/system/cpu/cpu{cpu}/topology/thread_siblings_list')
    return [c for c in cpu_list(path.read_text()) if c != cpu]


def cpu_snapshot(cpus):
    rows = {}
    for line in Path('/proc/stat').read_text().splitlines():
        fields = line.split()
        if fields[0] in {f'cpu{cpu}' for cpu in cpus}:
            # guest and guest_nice are already included in user and nice.
            ticks = [int(n) for n in fields[1:9]]
            rows[fields[0]] = {'total': sum(ticks), 'idle': ticks[3] + ticks[4]}
    if len(rows) != len(cpus):
        raise ValueError('SMT sibling missing from /proc/stat')
    return rows


def busy_share(before, after):
    shares = {}
    for cpu, start in before.items():
        total = after[cpu]['total'] - start['total']
        idle = after[cpu]['idle'] - start['idle']
        shares[cpu] = (total - idle) / total if total > 0 else None
    return shares


def sample(participant, cpu, timeout):
    """One process under capped; perf counts only the pinned participant tree."""
    sibling_cpus = siblings(cpu)
    env = {**os.environ, **participant.get('env', {}), 'LC_ALL': 'C'}
    with tempfile.TemporaryDirectory(prefix='micro-') as scratch:
        counters = Path(scratch) / 'perf.csv'
        with open(Path(scratch) / 'stdout', 'w+b') as output:
            with open(participant.get('stdin', '/dev/null'), 'rb') as source:
                before = cpu_snapshot(sibling_cpus)
                process = subprocess.run([
                    str(ROOT / 'tools/capped'), '--timeout', str(timeout),
                    'taskset', '-c', str(cpu), 'perf', 'stat', '--no-big-num', '-x', ';',
                    '-e', ','.join(EVENTS), '-o', str(counters), '--', *participant['command'],
                ], cwd=participant.get('cwd', ROOT), env=env, stdin=source,
                    stdout=output, stderr=subprocess.PIPE, text=True)
                after = cpu_snapshot(sibling_cpus)
            if process.returncode:
                raise ValueError(f"{participant['name']}: exit {process.returncode}\n{process.stderr}")
            counts, running = perf_counts(counters.read_text())
            output.seek(0)
            digest = hashlib.file_digest(output, 'sha256').hexdigest()
    return {'counts': counts, 'running_percent': running, 'stdout_sha256': digest,
            'sibling_before': before, 'sibling_after': after,
            'sibling_busy_share': busy_share(before, after)}


def schedule(names, rounds, seed):
    rng = random.Random(seed)
    order = []
    for round_number in range(rounds):
        shuffled = list(names)
        rng.shuffle(shuffled)
        order.extend((round_number, name) for name in shuffled)
    return order


def ratio(left, right):
    return statistics.median(left) / statistics.median(right)


def bootstrap_ratio(left, right, seed, draws=5000):
    """Paired-round bootstrap: preserve each round's shared conditions.

    This estimates within-session jitter only; it cannot bound host drift.
    """
    if len(left) != len(right) or not left:
        raise ValueError('bootstrap needs matching nonempty rounds')
    rng = random.Random(seed)
    values = []
    for _ in range(draws):
        indices = rng.choices(range(len(left)), k=len(left))
        values.append(ratio([left[i] for i in indices], [right[i] for i in indices]))
    values.sort()
    return [values[int(draws * 0.025)], values[min(draws - 1, int(draws * 0.975))]]


def comparisons(data, reference):
    names = [p['name'] for p in data['participants']]
    if reference not in names:
        raise ValueError(f'unknown reference: {reference}')
    counts = {name: [s['counts'] for s in sorted(data['samples'], key=lambda s: s['round'])
                     if s['name'] == name] for name in names}
    rows = []
    for name in names:
        own, base = counts[name], counts[reference]
        row = {'name': name, 'reference': reference}
        for event in ('instructions', 'instructions:u', 'cycles', 'task-clock'):
            left, right = [c[event] for c in own], [c[event] for c in base]
            row[event + '_ratio'] = ratio(left, right)
            if event == 'task-clock':
                row['task-clock_ratio_ci95'] = bootstrap_ratio(left, right, data['seed'])
        row['ipc'] = statistics.median(c['instructions'] / c['cycles'] for c in own)
        row['ipc:u'] = statistics.median(c['instructions:u'] / c['cycles:u'] for c in own)
        rows.append(row)
    return rows


def cross_session(current, previous):
    """Only instruction ratios are meaningful between saved sessions."""
    rows = []
    for p in current['participants']:
        name = p['name']
        for event in ('instructions', 'instructions:u'):
            old = [s['counts'][event] for s in previous['samples'] if s['name'] == name]
            new = [s['counts'][event] for s in current['samples'] if s['name'] == name]
            if old and new:
                rows.append({'name': name, 'metric': event, 'ratio': ratio(new, old)})
    if not rows:
        raise ValueError('no participants shared with the saved session')
    return rows


def timing_lock(cpu):
    """CPU31's convention lock spans the full session, never a capped slot."""
    path = TIMING_LOCK if cpu == 31 else Path(tempfile.gettempdir()) / f'hirsel-micro-cpu{cpu}.lock'
    lock = open(path, 'a+')
    fcntl.flock(lock, fcntl.LOCK_EX)
    return lock
