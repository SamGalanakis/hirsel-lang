"""Shared, bounded Buck2 event history for landing, main's hourly run and pool audits."""
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
            with path.open() as source:
                yield from source
            return
        with tempfile.TemporaryFile(mode='w+') as errors:
            proc = subprocess.Popen([str(buck2 or BUCK2), 'log', 'show', str(path.resolve())],
                                    cwd=ROOT, stdout=subprocess.PIPE, stderr=errors, text=True)
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
            profile = 'dev' if ':dev#' in profile or profile.startswith('hirsel-rust-host#') else 'release'
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


# Buck2's TestStatus (buck2_data.proto).
PASS, FAIL, SKIP, OMITTED, FATAL, TIMED_OUT = 1, 2, 3, 4, 5, 6


def test_results(event_log, *, history=None):
    """Each test's TestResult event from a `buck2 test` event log, by short
    name; the per-case costs go to the test history (test_costs.py)."""
    events = subprocess.run([str(BUCK2), 'log', 'show', str(event_log)], cwd=ROOT, check=True,
                            stdout=subprocess.PIPE, text=True, errors='replace').stdout
    results, rows = {}, []
    for line in events.splitlines():
        if '"TestResult"' not in line:
            continue
        event = json.loads(line).get('Event', {})
        result = event.get('data', {}).get('Instant', {}).get('data', {}).get('TestResult')
        if not result:
            continue
        stamp = event.get('timestamp')
        date = (datetime.fromtimestamp(stamp[0] + stamp[1] / 1e9, timezone.utc) if stamp
                else datetime.now(timezone.utc)).isoformat()
        name = result['name'].removeprefix('root//:').split(' ')[0]
        seconds = (result.get('duration_us') or 0) / 1e6
        details = result.get('details') or ''
        results[name] = dict(status=result.get('status'), details=details, seconds=seconds)
        match = re.fullmatch(r'(.+)-shard-(\d+)', name)
        costs = case_costs(details)
        rows.append(dict(gate=Path(event_log).name.removesuffix('.pb.zst'),
                         target=match[1] if match else name, shard=match[2] if match else '',
                         duration_seconds=seconds, date=date, status=result.get('status'),
                         case_costs=json.dumps(costs, sort_keys=True, separators=(',', ':')) if costs else ''))
    (test_history() if history is None else history).append(rows)
    return results


def run(arguments, *, buck2=None, runner=None, **kwargs):
    """Run Buck2, retaining every build/test unless its caller owns the log."""
    runner = runner or subprocess.run
    command = [str(buck2 or BUCK2), *arguments]
    options = list(arguments)
    options = options[:options.index('--')] if '--' in options else options
    if arguments and arguments[0] in ('build', 'test') and '--event-log' not in options:
        with recorded(arguments[0]) as path:
            return runner([command[0], command[1], '--event-log', str(path), *command[2:]], **kwargs)
    return runner(command, **kwargs)


if __name__ == '__main__':
    raise SystemExit(run(sys.argv[1:]).returncode)
