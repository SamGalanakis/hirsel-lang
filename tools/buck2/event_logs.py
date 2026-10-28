"""Shared, bounded Buck2 event history for landing, main's hourly run and pool audits."""
from concurrent.futures import ThreadPoolExecutor
from contextlib import contextmanager, nullcontext
import csv
from datetime import datetime, timezone
import fcntl
import json
import os
from pathlib import Path
import re
import subprocess
import tempfile
import sys
import time

ROOT = Path(__file__).resolve().parents[2]
BUCK2 = ROOT / '.buck2/bin/buck2'
MAX_AGE = 3 * 86400
MAX_BYTES = 4 * 1024**3
CASE_COST = re.compile(r'^test (\S+)(?: - should panic)? \.\.\. ok <(\d+(?:\.\d+)?)s>')


def case_costs(details):
    """Successful per-case timings in one libtest result (failures stay unpriced)."""
    rows = {}
    for line in details.splitlines():
        match = CASE_COST.match(line) or re.fullmatch(
            r'native case\s+(\S+)\s+([0-9.]+)\s+(?:[0-9]+|unknown)', line)
        if match:
            rows[match[1]] = float(match[2])
    return rows


class NoTestHistory:
    def append(self, rows):
        pass


class TsvTestHistory:
    """Append-only compact results, independently of compressed-log retention."""
    FIELDS = ('gate', 'target', 'shard', 'duration_seconds', 'date', 'status', 'case_costs')

    def __init__(self, path):
        self.path = Path(path).expanduser()

    def append(self, rows):
        if not rows:
            return
        self.path.parent.mkdir(parents=True, exist_ok=True)
        with self.path.open('a+', newline='') as out:
            fcntl.flock(out, fcntl.LOCK_EX)
            out.seek(0, os.SEEK_END)
            writer = csv.DictWriter(out, fieldnames=self.FIELDS, delimiter='\t', lineterminator='\n')
            if out.tell() == 0:
                writer.writeheader()
            writer.writerows(rows)


def test_history():
    """Host-selected file sink (default) or explicit opt-out; no fsync per row."""
    configured = os.environ.get('HIRSEL_TEST_HISTORY', str(Path.home() / '.cache/hirsel-buck2/test-results.tsv'))
    return NoTestHistory() if configured == 'none' else TsvTestHistory(configured)


def test_history_rows(path):
    """Read complete TSV rows while other gates may be appending large cost maps."""
    with Path(path).open(newline='') as source:
        fcntl.flock(source, fcntl.LOCK_SH)
        yield from csv.DictReader(source, delimiter='\t')


def directory():
    return Path(os.environ.get('HIRSEL_BUCK2_LOG_DIR',
                               str(Path.home() / '.cache/hirsel-buck2/event-logs'))).expanduser()


def lock_path(path):
    return path.with_name(path.name + '.lock')


@contextmanager
def reading(path):
    """Protect a retained log from rotation; skip a run that is still writing."""
    lock = lock_path(path)
    with (lock.open('r') if lock.exists() else nullcontext(None)) as handle:
        if handle:
            try:
                fcntl.flock(handle, fcntl.LOCK_SH | fcntl.LOCK_NB)
            except BlockingIOError:
                yield False
                return
        yield True


def prune(folder, *, now=None, max_age=MAX_AGE, max_bytes=MAX_BYTES):
    """Rotate oldest completed logs first, preserving running writers/readers."""
    now = time.time() if now is None else now
    folder.mkdir(parents=True, exist_ok=True)
    with (folder / '.rotation.lock').open('a+') as rotation:
        fcntl.flock(rotation, fcntl.LOCK_EX)
        files = sorted(folder.glob('*.pb.zst'), key=lambda p: p.stat().st_mtime)
        total = sum(p.stat().st_size for p in files)
        for path in files:
            stat = path.stat()
            if now - stat.st_mtime <= max_age and total <= max_bytes:
                continue
            with lock_path(path).open('a+') as handle:
                try:
                    fcntl.flock(handle, fcntl.LOCK_EX | fcntl.LOCK_NB)
                except BlockingIOError:
                    continue
                path.unlink()
                total -= stat.st_size
            lock_path(path).unlink(missing_ok=True)


@contextmanager
def recorded(kind, folder=None):
    """Buck2 writes its compressed log here directly, including failed runs."""
    folder = directory() if folder is None else Path(folder)
    folder.mkdir(parents=True, exist_ok=True)
    path = folder / f'{time.time_ns()}-{os.getpid()}-{kind}.pb.zst'
    try:
        with lock_path(path).open('a+') as handle:
            fcntl.flock(handle, fcntl.LOCK_EX)
            try:
                yield path
            finally:
                # Keep the lock file next to a completed log so audit readers
                # and rotation use the same inode. A crash releases the lock.
                if not path.exists():
                    lock_path(path).unlink(missing_ok=True)
    finally:
        prune(folder)


def discover(folder, hours, *, now=None):
    """Recent retained logs, ignoring temporary files and in-flight runs."""
    now = time.time() if now is None else now
    found = []
    for path in Path(folder).glob('*.pb.zst'):
        try:
            with reading(path) as ready:
                if ready and path.stat().st_mtime >= now - hours * 3600:
                    found.append(path)
        except FileNotFoundError:
            pass  # Rotation won the race before we acquired the read lock.
    return sorted(found)


def lines(path, buck2=None):
    """Read JSONL or stream Buck2's decoder for its native compressed format."""
    path = Path(path)
    with reading(path) as ready:
        if not ready:
            return
        if not path.name.endswith('.pb.zst'):
            with path.open(errors='replace') as source:
                yield from source
            return
        with tempfile.TemporaryFile(mode='w+') as errors:
            proc = subprocess.Popen([str(buck2 or BUCK2), 'log', 'show', str(path.resolve())],
                                    cwd=ROOT, stdout=subprocess.PIPE, stderr=errors, text=True, errors='replace')
            try:
                yield from proc.stdout
                status = proc.wait()
                if status:
                    errors.seek(0)
                    raise subprocess.CalledProcessError(status, proc.args, stderr=errors.read())
            finally:
                proc.stdout.close()
                if proc.poll() is None:
                    proc.terminate()
                    proc.wait()


def commands(paths):
    """Remote compile and test records; merged executions count once across logs."""
    seen = set()
    for path in paths:
        for line in lines(path):
            data = json.loads(line).get('Event', {}).get('data', {}).get('SpanEnd', {}).get('data', {})
            action = data.get('ActionExecution')
            test = data.get('TestRun')
            if action:
                owner = (action.get('key', {}).get('owner') or {}).get('TargetLabel', {})
                category, phase = action['name']['category'], action['name']['identifier']
                reports, failed = action.get('commands') or [], action.get('failed', False)
            elif test:
                owner = test['suite']['target_label']
                category, phase = 'test', test['suite']['suite_name']
                reports = [test['command_report']]
                failed = 'Success' not in test['command_report'].get('status', {})
            else:
                continue
            label = owner.get('label', {})
            if not label.get('package', '').startswith('root//'):
                continue
            target = label['name']
            display = target if label['package'] == 'root//' else label['package'] + ':' + target
            profile = owner.get('configuration', {}).get('full_name', '')
            profile = 'dev' if any(p in profile for p in (':dev#', ':checked#')) or profile.startswith('hirsel-rust-host#') else 'release'
            for report in reports:
                details = report.get('details', {})
                remote = details.get('command_kind', {}).get('command', {}).get('RemoteCommand')
                metadata = details.get('metadata') or {}
                if not remote or not metadata.get('start_time') or metadata.get('execution_time_us') is None:
                    continue
                start = metadata['start_time']
                # A cache hit carries the original execution's timing. It must
                # not hide a later-read miss for that execution in another log.
                key = (remote['action_digest'], tuple(start), metadata['execution_time_us'], remote['cache_hit'])
                if key in seen:
                    continue
                seen.add(key)
                wall = metadata['execution_time_us'] / 1e6
                props = {p['name']: p['value'] for p in remote['details']['platform']['properties']}
                yield dict(target=target, display=display, profile=profile, phase=phase,
                           category=category, digest=remote['action_digest'], cached=remote['cache_hit'],
                           failed=failed or details.get('signed_exit_code', 0) != 0,
                           wall=wall, end=start[0] + start[1] / 1e9 + wall,
                           cpu=int(props.get('cpu_count', 1)), memory_kb=int(props.get('memory_kb', 0)))


# Buck2's TestStatus (app/buck2_data/data.proto).
PASS, FAIL, SKIP, OMITTED, FATAL, TIMED_OUT = 1, 2, 3, 4, 5, 6
INFRA_FAILURE = 11


def test_results(event_log, *, history=None, all_results=False):
    """Each test's TestResult event from a `buck2 test` event log, by short
    name; the per-case costs go to the test history (test_costs.py).

    all_results retains repeated results and TestRuns missing their result.
    Provenance is unknown unless one owning TestRun supplies execution/cache
    evidence; absent/ambiguous command evidence must not imply a fresh pass.
    """
    results, rows, observed, executions = {}, [], [], {}
    pending = []
    # Decode once, retaining test records only. Build/action events can dwarf
    # the test output; never buffer Buck's entire JSON decoder output.
    for line in lines(event_log):
        event = json.loads(line).get('Event', {})
        result = event.get('data', {}).get('Instant', {}).get('data', {}).get('TestResult')
        if result:
            pending.append((event.get('timestamp'), result))
        run = event.get('data', {}).get('SpanEnd', {}).get('data', {}).get('TestRun')
        if not run:
            continue
        suite = run['suite']
        name = suite['target_label']['label']['name']
        report = run.get('command_report', {})
        details = report.get('details') or {}
        command = details.get('command_kind', {}).get('command', {})
        remote = command.get('RemoteCommand')
        execution = 'unknown'
        if remote is not None:
            execution = {True: 'cached', False: 'fresh'}.get(remote.get('cache_hit'), 'unknown')
        elif 'LocalCommand' in command:
            execution = 'fresh'
        executions.setdefault(name, []).append(dict(
            execution=execution, command_kind=next(iter(command), 'unknown'),
            digest=remote.get('action_digest') if remote else None,
            start_time=(details.get('metadata') or {}).get('start_time'),
            configuration=suite['target_label'].get('configuration'),
            suite=suite.get('suite_name'), report_status=report.get('status'),
            signed_exit_code=details.get('signed_exit_code'),
            command_succeeded='Success' in report.get('status', {}) and details.get('signed_exit_code', 0) == 0))
    # Results can precede their TestRun; resolve provenance after EOF.
    for stamp, result in pending:
        date = (datetime.fromtimestamp(stamp[0] + stamp[1] / 1e9, timezone.utc) if stamp
                else datetime.now(timezone.utc)).isoformat()
        name = result['name'].removeprefix('root//:').split(' ')[0]
        seconds = (result.get('duration_us') or 0) / 1e6
        details = result.get('details') or ''
        sources = executions.get(name, [])
        provenance = (sources[0] if len(sources) == 1 else dict(execution='unknown', executions=sources))
        decoded = dict(status=result.get('status'), details=details, seconds=seconds,
                       provenance=provenance | dict(event_timestamp=stamp))
        results[name] = decoded
        observed.append((name, decoded))
        match = re.fullmatch(r'(.+)-shard-(\d+)', name)
        costs = case_costs(details)
        rows.append(dict(gate=Path(event_log).name.removesuffix('.pb.zst'),
                         target=match[1] if match else name, shard=match[2] if match else '',
                         duration_seconds=seconds, date=date, status=result.get('status'),
                         case_costs=json.dumps(costs, sort_keys=True, separators=(',', ':')) if costs else ''))
    if all_results:
        for name, sources in executions.items():
            if name not in results:
                observed.append((name, dict(status=None, details='', seconds=0, missing_result=True,
                                           provenance=dict(execution='unknown', executions=sources))))
    (test_history() if history is None else history).append(rows)
    return observed if all_results else results


def test_batches(command, size):
    """Bound outstanding test requests, including their prepared input trees.

    Buck2 6507dd15's test runner buffers 10,000 requests (app/buck2_test_runner/
    src/runner.rs:104-106); the RE execution semaphore is acquired only after
    preparation (app/buck2_execute/src/re/client.rs:1478). Hundreds of queued
    shards therefore exhaust the scheduled run's 8 GiB before they execute.
    Keep every explicit target and runner option, with a bounded number per command.
    """
    if not command or command[0] != 'test':
        yield command
        return
    end = command.index('--') if '--' in command else len(command)
    targets = [i for i in range(1, end) if command[i].startswith(('//:', 'root//:'))]
    if len(targets) <= size:
        yield command
        return
    # Stage constructors put all explicit targets together after Buck options.
    if targets != list(range(targets[0], targets[-1] + 1)):
        raise ValueError('test targets must be contiguous for bounded execution')
    for start in range(targets[0], targets[-1] + 1, size):
        yield command[:targets[0]] + command[start:min(start + size, targets[-1] + 1)] + command[targets[-1] + 1:]


def run(arguments, *, buck2=None, runner=None, batch_size=None, batch_concurrency=None, **kwargs):
    """Run Buck2, retaining every build/test unless its caller owns the log."""
    runner = runner or subprocess.run
    size = int(os.environ.get('HIRSEL_TEST_BATCH_SIZE', '0')) if batch_size is None else batch_size
    if size < 0:
        raise ValueError('HIRSEL_TEST_BATCH_SIZE must be nonnegative')
    concurrency = int(os.environ.get('HIRSEL_TEST_BATCH_CONCURRENCY', '1')) if batch_concurrency is None else batch_concurrency
    if concurrency < 1:
        raise ValueError('HIRSEL_TEST_BATCH_CONCURRENCY must be positive')
    if size and arguments and arguments[0] == 'test':
        workers = min(concurrency, size)
        batches = test_batches(arguments, max(1, size // workers))
        def execute(batch):
            return run(batch, buck2=buck2, runner=runner, batch_size=0, **kwargs)
        # Up to `size` prepared requests, replenished in small groups instead
        # of waiting for the slowest of 32 before admitting any next test.
        with ThreadPoolExecutor(max_workers=workers) as pool:
            results = list(pool.map(execute, batches))
        def combined(stream):
            values = [getattr(result, stream, None) for result in results]
            values = [value for value in values if value is not None]
            return (values[0][:0].join(values) if values else None)
        return subprocess.CompletedProcess([str(buck2 or BUCK2), *arguments],
                                           next((r.returncode for r in results if r.returncode), 0),
                                           combined('stdout'), combined('stderr'))
    command = [str(buck2 or BUCK2), *arguments]
    options = list(arguments)
    options = options[:options.index('--')] if '--' in options else options
    if arguments and arguments[0] in ('build', 'test') and '--event-log' not in options:
        with recorded(arguments[0]) as path:
            return runner([command[0], command[1], '--event-log', str(path), *command[2:]], **kwargs)
    return runner(command, **kwargs)


if __name__ == '__main__':
    raise SystemExit(run(sys.argv[1:]).returncode)
