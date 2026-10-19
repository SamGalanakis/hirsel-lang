"""What tools/gate and tools/land share: a change's paths and identity, the
selection of what it can affect (impact.py), one Buck2 invocation for all of
it, the green record and the failure summary."""

from __future__ import annotations

from concurrent.futures import ThreadPoolExecutor
from dataclasses import dataclass, field
from datetime import datetime, timezone
import contextlib
import fcntl
import json
import os
from pathlib import Path
import re
import shlex
import signal
import subprocess
import sys
import tempfile
import threading
import time
import tomllib

import impact
import event_logs
import graph
from sync import (AGREE_LIMIT, AGREE_SHARDS, AGREE_SLICE_PROGRAMS, CONFORMANCE_SHARDS, CONFORMANCE_SLICE_PROGRAMS,
                  SELECTED_SLICES)

# tools/land runs main's copy of these scripts on a lane's worktree (HIRSEL_LAND_TREE).
ROOT = Path(os.environ.get('HIRSEL_LAND_TREE') or Path(__file__).resolve().parents[2])
BUCK2 = ROOT / '.buck2/bin/buck2'
# Every Buck2 command of the per-push gate and of land, so the daemon keeps one
# state: the Clippy twins exist, and `buck2 test` also builds the non-test
# targets it is given, which puts the build, Clippy and the tests in one
# invocation. Neither option changes any other action's key.
CONFIG = ['-c', 'hirsel.clippy=true', '-c', 'buck2.test_builds_targets=true']
TIMEOUT = 1800

ALL_TESTS = '(kind("_rust_test", //...) + attrfilter(labels, doctest, //...)) - attrfilter(labels, gate, //...)'
BUILD_SET = ('root//: - kind("_rust_test", root//:) - attrregexfilter(name, "-clippy$", root//:)'
             ' - attrfilter(labels, gate, root//:)')
CLIPPY_SET = 'attrregexfilter(name, "-clippy$", kind("^rust_(library|binary)$", root//:))'
CAMPAIGN_TARGETS = {
    impact.CONFORMANCE: [f'root//:gate-conformance-shard-{i}' for i in range(1, CONFORMANCE_SHARDS + 1)],
    impact.AGREEMENT: [f'root//:gate-agree-shard-{i}' for i in range(1, AGREE_SHARDS + 1)],
    impact.NATIVE_SMOKE: ['root//:gate-native-smoke', 'root//tools/buck2/native-toolchain:proof',
                          'root//:gate-link-hosts'],
}

# A file carrying this marker keeps its own test binary, for example to install
# a global allocator; its first lines say why.
OWN_BINARY = 'own test binary:'
# Tests that compile source through the driver live in hirsel-pipeline-tests, so an
# implementation crate's tests are not selected by every edit under the driver.
# Only these crates may depend on the driver, or on its engine, which the
# driver alone wires.
DRIVER = 'hirsel-driver'
ENGINE = 'hirsel-engine'
DRIVER_DEPENDENTS = {
    'hirsel-cli': 'the command-line front end',
    'hirsel-conformance': 'the conformance, agreement and native runners',
    'hirsel-pipeline-tests': 'implementation-crate tests that compile source',
    'hirsel-native-build': 'the native benchmark build helper',
    # These unit-test harnesses read the core library
    # signatures from BuiltinRegistry, which is built with check and modes.
    # hirsel-signature and hirsel-lower have no registry-backed harnesses.
    'hirsel-check': 'body-check harnesses read BuiltinRegistry signatures',
    'hirsel-modes': 'authority and ownership harnesses read BuiltinRegistry signatures',
}


def say(message):
    print(message, flush=True)


# ---------------------------------------------------------------- daemon lifetime


# Helpers (prepare, formatting and full-gate scripts) can themselves start
# Buck2. Reap their process groups before the final kill so no helper can
# restart a daemon after the lane exits.
COMMANDS = set()
COMMAND_LOCK = threading.RLock()
STOPPING = False


# A helper that cleans up after itself (land's batch runner stops its own
# commands and daemons) gets this long after SIGTERM before SIGKILL.
STOP_GRACE_SECONDS = 10


def signal_command(process, sig):
    try:
        os.killpg(process.pid, sig)
    except ProcessLookupError:
        pass


def stop_commands(processes):
    """SIGTERM each command's process group, then SIGKILL what outlives the grace."""
    for process in processes:
        signal_command(process, signal.SIGTERM)
    deadline = time.monotonic() + STOP_GRACE_SECONDS
    for process in processes:
        try:
            process.wait(max(0, deadline - time.monotonic()))
        except subprocess.TimeoutExpired:
            pass
        signal_command(process, signal.SIGKILL)
        process.wait()


def run(command, *, capture_output=False, check=False, input=None, timeout=None, **kwargs):
    """subprocess.run with ownership of helper descendants on interruption."""
    if capture_output:
        kwargs.update(stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    if input is not None:
        kwargs['stdin'] = subprocess.PIPE
    with COMMAND_LOCK:
        if STOPPING:
            raise SystemExit('lane is stopping')
        process = subprocess.Popen(command, start_new_session=True, **kwargs)
        COMMANDS.add(process)
    try:
        with process:
            try:
                stdout, stderr = process.communicate(input, timeout=timeout)
            except BaseException:
                stop_commands([process])
                raise
            result = subprocess.CompletedProcess(command, process.returncode, stdout, stderr)
            if check:
                result.check_returncode()
            return result
    finally:
        with COMMAND_LOCK:
            COMMANDS.discard(process)


def stop_daemons():
    """Release this worktree's daemon memory, including preparation's isolation dir.

    Commands use the default isolation dir (or Buck2's environment override).
    Prelude expansion uses buck2-bootstrap. Kill both even after a failed or
    interrupted prepare; kill never starts a daemon.
    """
    if os.environ.get('HIRSEL_KEEP_DAEMON') == '1' or not BUCK2.exists():
        return
    errors = []
    for isolation in (None, 'buck2-bootstrap'):
        command = [str(BUCK2)]
        if isolation is not None:
            command += ['--isolation-dir', isolation]
        result = subprocess.run([*command, 'kill'], cwd=ROOT, capture_output=True, text=True)
        if result.returncode:
            errors.append(result.stderr.strip())
    if errors:
        raise RuntimeError('Buck2 daemon cleanup failed: ' + '; '.join(errors))


@contextlib.contextmanager
def daemon_lifetime():
    """Make normal exits, exceptions and catchable termination release daemons."""
    global STOPPING
    STOPPING = False
    signals = (signal.SIGINT, signal.SIGTERM, signal.SIGHUP)
    previous = {sig: signal.getsignal(sig) for sig in signals}

    def interrupted(signum, frame):
        raise SystemExit(128 + signum)

    try:
        for sig in signals:
            signal.signal(sig, interrupted)
        yield
    finally:
        # A repeated interrupt must not interrupt the kill itself.
        for sig in signals:
            signal.signal(sig, signal.SIG_IGN)
        try:
            with COMMAND_LOCK:
                STOPPING = True
                stop_commands(list(COMMANDS))
            stop_daemons()
        finally:
            for sig, handler in previous.items():
                signal.signal(sig, handler)


# ---------------------------------------------------------------- gate slots

# The host's slot directory: its `count` and `express-count` files cap normal
# and express gates and are re-read every poll (tools/gate --set-slots). The
# count is the host's, never a process's: a `:N` in HIRSEL_GATE_SLOTS is
# ignored, so a stale value in a brief or a shell cannot strand slots.
# HIRSEL_GATE_SLOTS=<dir> selects another directory and `none` runs uncapped.
GATE_SLOTS = 'HIRSEL_GATE_SLOTS'
GATE_SLOT_DIR = Path('/workspace/code/hirsel-briefs/locks/gate-slots')
NO_SLOTS = 'none'
SLOT_POLL_SECONDS = 5
NORMAL_POOL = 'normal'
EXPRESS_POOL = 'express'
# What seeds a missing count file.
DEFAULT_SLOTS = {NORMAL_POOL: 6, EXPRESS_POOL: 2}
# One selected conformance slice and one agreement slice, plus two test targets.
EXPRESS_TEST_TARGETS = 4
POOL_PREFIX = {NORMAL_POOL: '', EXPRESS_POOL: 'express-'}


def gate_slot_dir(environ):
    """The host's slot directory, or None for no cap: HIRSEL_GATE_SLOTS names
    another directory (`dir`, or `dir:N` with N ignored) or `none`; unset, the
    host default when its parent exists."""
    spec = environ.get(GATE_SLOTS)
    if spec is None:
        return GATE_SLOT_DIR if GATE_SLOT_DIR.parent.is_dir() else None
    if spec == NO_SLOTS:
        return None
    directory, colon, count = spec.rpartition(':')
    if colon and count.isascii() and count.isdigit():
        spec = directory
    if not spec:
        raise SystemExit(f'gate: {GATE_SLOTS}={environ[GATE_SLOTS]!r} names no directory (dir, or {NO_SLOTS})')
    return Path(spec)


def write_gate_slot_count(directory, count, *, pool=NORMAL_POOL, seed=False):
    """Publish a complete count atomically; seeding never replaces a host's cap."""
    directory.mkdir(parents=True, exist_ok=True)
    name = POOL_PREFIX[pool] + 'count'
    path = directory / name
    with tempfile.NamedTemporaryFile('w', dir=directory, prefix=f'.{name}-', delete=False) as temporary:
        pending = Path(temporary.name)
        try:
            temporary.write(f'{count}\n')
            temporary.flush()
            if seed:
                try:
                    os.link(pending, path)
                except FileExistsError:
                    pass
            else:
                pending.replace(path)
        finally:
            pending.unlink(missing_ok=True)


def read_gate_slot_count(directory, fallback=None, *, pool=NORMAL_POOL):
    path = directory / (POOL_PREFIX[pool] + 'count')
    try:
        text = path.read_text().strip()
    except FileNotFoundError:
        return DEFAULT_SLOTS[pool] if fallback is None else fallback
    if not text.isascii() or not text.isdigit() or int(text) < 1:
        raise SystemExit(f'gate: {path}={text!r} is not a positive slot count')
    return int(text)


def process_start(pid):
    """Linux process identity survives PID reuse; zombies cannot keep a ticket."""
    try:
        fields = Path(f'/proc/{pid}/stat').read_text().rsplit(')', 1)[1].split()
    except (FileNotFoundError, ProcessLookupError):
        return None
    return None if fields[0] == 'Z' else int(fields[19])


def live_tickets(queue):
    """The queue's live waiters, earliest first, as (stamp, pid, ticket);
    tickets left by exited gates are discarded."""
    live = []
    for ticket in queue.iterdir() if queue.is_dir() else ():
        try:
            stamp, pid = map(int, ticket.name.split('-'))
        except ValueError:
            continue
        if pid <= 0:
            continue
        try:
            start = ticket.read_text().strip()
        except FileNotFoundError:
            continue
        if start:
            current = process_start(pid)
            if current is None or str(current) != start:
                ticket.unlink(missing_ok=True)
                continue
        try:
            os.kill(pid, 0)
        except ProcessLookupError:
            ticket.unlink(missing_ok=True)
            continue
        except PermissionError:
            pass  # A live process owned by another user.
        live.append((stamp, pid, ticket))
    return sorted(live)


def oldest_gate_ticket(queue):
    """The earliest live waiter's ticket."""
    live = live_tickets(queue)
    return live[0][2] if live else None


def process_cwd(pid):
    """A process's working directory: a gate's or a land's worktree."""
    try:
        return os.readlink(f'/proc/{pid}/cwd')
    except OSError:
        return '?'


def holder(path):
    """A slot file's live holder as (pid, worktree, acquired), or None when free."""
    try:
        fields = path.read_text().split()
        acquired = path.stat().st_mtime
    except (OSError, ValueError):
        return None
    if len(fields) < 2 or not fields[0].isdigit() or str(process_start(int(fields[0]))) != fields[1]:
        return None
    # A live process may have released the slot: probe the lock without waiting.
    # A gate that meets the probe moves on to the next slot or the next poll.
    try:
        with open(path) as handle:
            fcntl.flock(handle, fcntl.LOCK_SH | fcntl.LOCK_NB)
        return None
    except BlockingIOError:
        return int(fields[0]), (fields[2] if len(fields) > 2 else '?'), acquired
    except OSError:
        return None


def minutes(seconds):
    return f'{seconds / 60:.0f} min' if seconds >= 60 else f'{seconds:.0f} s'


def queue_status(directory, *, pool=NORMAL_POOL, label='gate', me=(), now=None):
    """A pool's cap, its holders with their age, and its queue in order, as lines.
    `me` are this lane's worktrees: their holders and waiters are marked."""
    now = time.time() if now is None else now
    prefix = POOL_PREFIX[pool]
    description = 'land lock' if label == 'land' else f'{pool} gate slots'
    count = read_gate_slot_count(directory, pool=pool) if label != 'land' else 1
    slots = sorted((int(path.name.rpartition('-')[2]), path) for path in directory.glob(prefix + 'slot-*')
                   if path.name.rpartition('-')[2].isdigit())
    held = [(slot, found) for slot, path in slots if (found := holder(path))]
    waiting = [(stamp, pid, process_cwd(pid)) for stamp, pid, _ in live_tickets(directory / (prefix + 'queue'))]
    lines = [f'{label}: {description}: {count} in {directory}; {len(held)} held, {len(waiting)} queued']
    for slot, (pid, worktree, acquired) in held:
        retired = ' (retired by a lowered count)' if slot >= count else ''
        mine = ' (you)' if worktree in me else ''
        lines.append(f'  slot {slot}{retired}: held {minutes(now - acquired)} by {worktree} (pid {pid}){mine}')
    for position, (stamp, pid, worktree) in enumerate(waiting, 1):
        mine = ' (you)' if worktree in me else ''
        lines.append(f'  queued {position}: waiting {minutes(now - stamp / 1e9)}, {worktree} (pid {pid}){mine}')
    for slot, (pid, worktree, acquired) in held:
        if worktree in me:
            lines.append(f'{label}: you hold {description.removesuffix("s")} {slot}')
    positions = [position for position, (_, _, worktree) in enumerate(waiting, 1) if worktree in me]
    if positions:
        lines.append(f'{label}: you are at {positions[0]} of {len(waiting)} in the {description.removesuffix("s")} queue')
    return lines


@dataclass
class SlotTiming:
    wait_seconds: float
    held_seconds: float = 0


@contextlib.contextmanager
def gate_slot(environ=os.environ, *, pool=NORMAL_POOL, stamp=None):
    """Hold one slot in the selected host pool, in its own FIFO waiter order;
    `stamp` (nanoseconds) is the place a restarted gate keeps."""
    directory = gate_slot_dir(environ)
    if directory is None:
        yield None
        return
    with fifo_slot(directory, DEFAULT_SLOTS[pool], pool=pool, stamp=stamp) as timing:
        yield timing


@contextlib.contextmanager
def fifo_slot(directory, fallback, *, pool=NORMAL_POOL, label='gate', exclusive=False, stamp=None):
    """Share the file-lock FIFO between gates and the exclusive land queue.

    flock releases a crashed holder's slot without unlinking its inode. Tickets
    include the process start time so a reused PID cannot strand the queue.
    Legacy gate tickets remain readable while already-loaded gates finish.
    The queue orders tickets by `stamp`, the time of the first queueing, so
    a waiter given an earlier stamp keeps that place.
    """
    prefix = POOL_PREFIX[pool]
    if not (directory / (prefix + 'count')).exists():
        write_gate_slot_count(directory, fallback, pool=pool, seed=True)
    queue = directory / (prefix + 'queue')
    queue.mkdir(exist_ok=True)
    # Publish identity and ticket together, retaining names old gates understand.
    with tempfile.NamedTemporaryFile('w', dir=directory, prefix='.ticket-') as pending:
        pending.write(f'{process_start(os.getpid())}\n')
        pending.flush()
        stamp = time.time_ns() if stamp is None else stamp
        while True:
            ticket = queue / f'{stamp}-{os.getpid()}'
            try:
                os.link(pending.name, ticket)
                break
            except FileExistsError:
                stamp += 1
    started = time.monotonic()
    waiting = False
    description = 'land lock' if exclusive else f'{pool} gate slot'
    try:
        while True:
            count = read_gate_slot_count(directory, fallback, pool=pool)
            if exclusive and count != 1:
                raise SystemExit(f'{label}: {directory / "count"} must be 1 for exclusive landing')
            if oldest_gate_ticket(queue) == ticket:
                for slot in range(count):
                    # Python opens files non-inheritable, so Buck2 never holds the lock.
                    handle = open(directory / f'{prefix}slot-{slot}', 'a+')
                    try:
                        fcntl.flock(handle, fcntl.LOCK_EX | fcntl.LOCK_NB)
                    except BlockingIOError:
                        handle.close()
                        continue
                    with handle:
                        handle.truncate(0)
                        # The worktree, not ROOT: land runs main's copy of the scripts.
                        handle.write(f'{os.getpid()} {process_start(os.getpid())} {os.getcwd()}\n')
                        handle.flush()
                        ticket.unlink(missing_ok=True)
                        acquired = time.monotonic()
                        timing = SlotTiming(acquired - started)
                        say(f'{label}: took {description} {slot} after waiting {timing.wait_seconds:.3f} s')
                        try:
                            yield timing
                        finally:
                            timing.held_seconds = time.monotonic() - acquired
                            if exclusive:
                                say(f'{label}: held {description} for {timing.held_seconds:.3f} s')
                    return
            if not waiting:
                holders = [path.read_text().strip() if path.exists() else ''
                           for slot in range(count)
                           for path in [directory / f'{prefix}slot-{slot}']]
                say(f'{label}: waiting for one of {count} {description}s in {directory}; held by: ' + '; '.join(holders))
                waiting = True
            time.sleep(SLOT_POLL_SECONDS)
    finally:
        ticket.unlink(missing_ok=True)


# ---------------------------------------------------------------- git

def git(*arguments, root=ROOT, **kwargs) -> str:
    return subprocess.run(['git', *arguments], cwd=root, check=True, capture_output=True,
                          text=True, **kwargs).stdout


def merge_base(base, root=ROOT) -> str:
    return git('merge-base', 'HEAD', base, root=root).strip()


def changed_paths(base, root=ROOT) -> list[str]:
    """Paths the worktree changes from `base` (a commit), committed or not,
    tracked or untracked. Main may advance while a gate runs, so `base` is the
    lane's merge base, never main itself."""
    paths = []
    for arguments in (['diff', '--no-renames', '--name-only', '-z', base],
                      ['ls-files', '--others', '--exclude-standard', '-z']):
        paths.extend(os.fsdecode(path) for path in subprocess.check_output(
            ['git', *arguments], cwd=root).split(b'\0') if path)
    return sorted(set(paths))


def worktree_tree(root=ROOT) -> str:
    """The tree object of the worktree as it is, untracked files included."""
    with tempfile.TemporaryDirectory() as scratch:
        env = os.environ | {'GIT_INDEX_FILE': str(Path(scratch) / 'index')}
        git('read-tree', 'HEAD', root=root, env=env)
        git('add', '-A', '.', root=root, env=env)
        return git('write-tree', root=root, env=env).strip()


# Pathspec leaving the generated API index out of change identities.
GENERATED_INDEX_EXCLUDE = ':(exclude)docs/api.md'


def change_identity(base, root=ROOT) -> str:
    """The patch-id of the change from `base` to the worktree, from a diff without
    context lines: a clean rebase onto a newer main keeps it, and any edit of the
    change, including a hand-resolved conflict, gives a new one. Empty when the
    worktree does not differ from `base`. The generated docs/api.md is left out:
    the gate's drift test still checks it and tools/land regenerates it on the
    rebased tree, so regenerating it after a rebase keeps a passed gate."""
    diff = subprocess.run(['git', 'diff', '-U0', '--binary', '--no-renames', base, worktree_tree(root),
                           '--', '.', GENERATED_INDEX_EXCLUDE],
                          cwd=root, check=True, capture_output=True).stdout
    if not diff:
        return ''
    out = subprocess.run(['git', 'patch-id', '--stable'], cwd=root, input=diff, check=True,
                         capture_output=True).stdout.decode()
    return out.split()[0]


def green_path(root=ROOT) -> Path:
    # Per worktree: `git rev-parse --git-path` resolves inside this worktree's git dir.
    return root / git('rev-parse', '--git-path', 'hirsel-gate-green.json', root=root).strip()


def green_records(root=ROOT) -> dict:
    try:
        return json.loads(green_path(root).read_text())
    except (OSError, ValueError):
        return {}


def record_gate(identity, crates, root=ROOT, *, passed=True, **record):
    """Save a gate receipt, including partial results that never authorize land."""
    records = green_records(root)
    records[identity] = dict(record, passed=passed, crates=sorted(crates),
                             at=time.strftime('%Y-%m-%dT%H:%M:%SZ', time.gmtime()))
    path = green_path(root)
    temporary = path.with_name(path.name + f'.{os.getpid()}')
    temporary.write_text(json.dumps(records, indent=1, sort_keys=True) + '\n')
    temporary.replace(path)


def latest_green(root=ROOT):
    """The most recently recorded passed change of this worktree: (identity, record)."""
    records = [(record.get('at', ''), identity, record) for identity, record in green_records(root).items()
               if record.get('passed', True)]
    return max(records)[1:] if records else (None, None)


HUNK = re.compile(r'^@@ [^@]* @@', re.M)


def path_digests(base, root=ROOT) -> dict[str, str]:
    """Each changed path's diff from `base` to the worktree, hashed without its
    line numbers or blob ids: a clean rebase keeps every digest, and an edit
    changes only the edited paths'. The generated docs/api.md is left out."""
    import hashlib
    diff = subprocess.run(['git', 'diff', '-U0', '--binary', '--no-renames', base, worktree_tree(root),
                           '--', '.', GENERATED_INDEX_EXCLUDE],
                          cwd=root, check=True, capture_output=True).stdout.decode(errors='surrogateescape')
    digests = {}
    for part in re.split(r'^diff --git ', diff, flags=re.M)[1:]:
        header, _, body = part.partition('\n')
        name = header[2:2 + (len(header) - 5) // 2]
        body = '\n'.join(line for line in body.splitlines() if not line.startswith('index '))
        digests[name] = hashlib.sha256(HUNK.sub('@@', body).encode(errors='surrogateescape')).hexdigest()
    return digests


def changed_since(record, digests) -> list[str]:
    """The paths whose change differs from a recorded passed change's."""
    recorded = record.get('digests', {})
    return sorted(path for path in recorded.keys() | digests.keys() if recorded.get(path) != digests.get(path))


def is_green(identity, crates, root=ROOT) -> bool:
    record = green_records(root).get(identity)
    return (bool(identity) and record is not None and record.get('passed', True)
            and record['crates'] == sorted(crates))


# ---------------------------------------------------------------- workspace

def members(root=ROOT) -> set[str]:
    """Workspace member directory names under crates/ (each is its package name)."""
    workspace = tomllib.loads((root / 'Cargo.toml').read_text())['workspace']
    return {manifest.parent.name for pattern in workspace['members']
            for manifest in root.glob(pattern + '/Cargo.toml')}


def workspace_metadata(root=ROOT):
    """The members and their declared dependencies, in `cargo metadata`'s shape."""
    workspace = tomllib.loads((root / 'Cargo.toml').read_text())['workspace']
    manifests = sorted({m for pattern in workspace['members'] for m in root.glob(pattern + '/Cargo.toml')})
    packages = []
    for manifest in manifests:
        data = tomllib.loads(manifest.read_text())
        tables = [(data.get(key, {}), kind) for key, kind in
                  [('dependencies', None), ('dev-dependencies', 'dev'), ('build-dependencies', 'build')]]
        for target in data.get('target', {}).values():
            tables += [(target.get(key, {}), kind) for key, kind in
                       [('dependencies', None), ('dev-dependencies', 'dev'), ('build-dependencies', 'build')]]
        name = data['package']['name']
        packages.append(dict(id=name, name=name, dependencies=[
            dict(name=(spec.get('package', dependency) if isinstance(spec, dict) else dependency), kind=kind)
            for table, kind in tables for dependency, spec in table.items()]))
    return {'workspace_members': [p['id'] for p in packages], 'packages': packages}


def driver_dependents(metadata):
    """Workspace crates outside the allowed set that depend on the driver or the
    engine, of any kind. The driver itself is the engine's one wiring crate."""
    workspace = set(metadata['workspace_members'])
    return sorted(p['name'] for p in metadata['packages']
                  if p['id'] in workspace and p['name'] not in DRIVER_DEPENDENTS
                  and any(d['name'] == DRIVER or (d['name'] == ENGINE and p['name'] != DRIVER)
                          for d in p.get('dependencies', [])))


def campaign_table_errors(metadata, table=None) -> list[str]:
    """Rows of impact.CRATE_CAMPAIGNS missing a campaign that a crate in the
    normal dependency closure of that campaign's crates must select."""
    table = impact.CRATE_CAMPAIGNS if table is None else table
    workspace = set(metadata['workspace_members'])
    depends = {p['name']: {d['name'] for d in p.get('dependencies', []) if d['kind'] is None and d['name'] in workspace}
               for p in metadata['packages'] if p['id'] in workspace}
    errors = []
    for campaign, roots in impact.CAMPAIGN_ROOTS.items():
        seen, frontier = set(), [root for root in roots if root in depends]
        while frontier:
            crate = frontier.pop()
            if crate not in seen:
                seen.add(crate)
                frontier.extend(depends.get(crate, ()))
        errors += [f'{crate} is in the closure of {", ".join(roots)}; its row needs {campaign}'
                   for crate in sorted(seen) if crate in table and campaign not in table[crate]]
    return errors


def stray_test_binaries(root=ROOT):
    """Top-level tests/*.rs files of crates whose integration tests link as tests/it."""
    stray = []
    for main in sorted(Path(root).glob('crates/*/tests/it/main.rs')):
        for path in sorted(main.parent.parent.glob('*.rs')):
            if OWN_BINARY not in path.read_text():
                stray.append(path.relative_to(root).as_posix())
    return stray


def policy_errors(root=ROOT) -> list[str]:
    """Workspace rules the selection depends on."""
    errors = []
    if stray := stray_test_binaries(root):
        errors.append('each of these links the whole dependency graph as its own binary; declare it '
                      'as a module of tests/it/main.rs, or say why it needs its own binary with a '
                      f'"// {OWN_BINARY} <reason>" comment: ' + ', '.join(stray))
    if dependents := driver_dependents(workspace_metadata(root)):
        errors.append('these crates depend on hirsel-driver or hirsel-engine, so every edit under them '
                      'would select their tests; move the tests that compile source through the driver '
                      'to crates/hirsel-pipeline-tests: ' + ', '.join(dependents))
    crates = members(root)
    errors += ['tools/buck2/impact.py CRATE_CAMPAIGNS: ' + error for error in campaign_table_errors(workspace_metadata(root))]
    if missing := sorted(crates - impact.CRATE_CAMPAIGNS.keys()):
        errors.append('tools/buck2/impact.py CRATE_CAMPAIGNS has no row for: ' + ', '.join(missing))
    if stale := sorted(impact.CRATE_CAMPAIGNS.keys() - crates):
        errors.append('tools/buck2/impact.py CRATE_CAMPAIGNS names crates that are gone: ' + ', '.join(stale))
    return errors


def owned_paths(paths, root=ROOT):
    """The files to ask Buck2 about: a deleted file stands for one surviving file
    of its directory, which the same globs own, so the query stays small however
    many files the directory holds."""
    files = set()
    for path in paths:
        if (root / path).is_file():
            files.add(path)
            continue
        directory = (root / path).parent
        while not directory.is_dir():
            directory = directory.parent
        survivors = sorted(p for p in directory.iterdir() if p.is_file())
        if survivors:
            files.add(survivors[0].relative_to(root).as_posix())
    return sorted(files)


# ---------------------------------------------------------------- Buck2

# Settings one gate adds to every Buck2 command it runs (its chosen corpus
# cases, Plan.config), so the daemon keeps one state for the whole gate. Buck2
# reads them from an argfile: a setting may exceed the operating system's
# per-argument limit.
SESSION_CONFIG: list[str] = []


def buck2(command, *arguments, **kwargs):
    if not SESSION_CONFIG:
        return event_logs.run([command, *CONFIG, *arguments], buck2=BUCK2, runner=run, cwd=ROOT, **kwargs)
    with tempfile.NamedTemporaryFile('w', prefix='hirsel-config-', suffix='.args') as argfile:
        argfile.write('\n'.join(SESSION_CONFIG) + '\n')
        argfile.flush()
        return event_logs.run([command, *CONFIG, f'@{argfile.name}', *arguments], buck2=BUCK2, runner=run, cwd=ROOT, **kwargs)


def uquery(expression) -> list[str]:
    # Buck2 reads the query from an argfile itself, so a query of any length
    # never meets the operating system's per-argument limit.
    with tempfile.NamedTemporaryFile('w', prefix='hirsel-uquery-', suffix='.args') as argfile:
        argfile.write(expression + '\n')
        argfile.flush()
        result = buck2('uquery', f'@{argfile.name}', capture_output=True, text=True)
    if result.returncode:
        raise SystemExit(f'buck2 uquery failed: {expression}\n{result.stderr}')
    return result.stdout.split()


# What `tools/gate --tests` takes, beyond Buck2 test targets.
SUGGESTED = 'suggested'
ALL = 'all-tests'
WHOLE = {'conformance': impact.CONFORMANCE, 'agreement': impact.AGREEMENT, 'native-smoke': impact.NATIVE_SMOKE}
CONFORMANCE_PREFIX, AGREEMENT_PREFIX, NATIVE_PREFIX = 'conformance:', 'agreement:', 'native:'
# The CLI's end-to-end examples: each builds and runs a program on both
# backends, so they run when the CLI, the driver or an example changes.
EXAMPLES = 'hirsel-cli-test-examples'
EXAMPLES_INPUTS = ('crates/hirsel-cli/', 'crates/hirsel-driver/', 'examples/')
# A selection of more corpus cases than this runs the whole campaign instead.
WHOLE_CORPUS_SHARE = 0.5


def labels(names) -> set[str]:
    """Root target labels for graph names."""
    return {f'root//:{name}' for name in names}


def corpus_cases(prefix, root=ROOT) -> list[str]:
    """The corpus cases under a repository path prefix (a case names itself)."""
    cases = sorted(p.relative_to(root).as_posix() for p in (root / 'conformance').rglob('*.hir'))
    return [case for case in cases if case.startswith(prefix)]


def generated_program_id(program: str) -> bool:
    """A canonical, reproducible native::generate::from_id witness."""
    match = re.fullmatch(
        r'generated/(?:success|overflow|underflow|dividebyzero|index|requires)-(0|[1-9][0-9]{0,19})', program)
    return bool(match and int(match[1]) < 1 << 64)


def accepted(case, root=ROOT) -> bool:
    """Whether agreement mutates this case: it skips cases expected to reject."""
    try:
        header = (root / case).read_text(errors='replace').split('\n', 8)
    except OSError:
        return False
    return not any(line.startswith('// expect: reject') for line in header)


def slices(cases, smallest) -> list[list[str]]:
    """Cases dealt round-robin over up to SELECTED_SLICES slices of at least
    `smallest` cases each."""
    count = min(SELECTED_SLICES, -(-len(cases) // smallest))
    return [cases[index::count] for index in range(count)]


@dataclass
class Plan:
    """What one gate runs: Buck2 tests, whole campaigns, chosen corpus cases
    for conformance and for agreement's seed-1 sample, and native programs."""
    tests: set = field(default_factory=set)
    campaigns: set = field(default_factory=set)
    conformance: set = field(default_factory=set)
    agreement: set = field(default_factory=set)
    native: set = field(default_factory=set)
    # (token, why) for every part, in the order they were chosen.
    reasons: list = field(default_factory=list)

    def add(self, other: 'Plan'):
        self.tests |= other.tests
        self.campaigns |= other.campaigns
        self.conformance |= other.conformance
        self.agreement |= other.agreement
        self.native |= other.native
        self.reasons += [r for r in other.reasons if r not in self.reasons]

    def settle(self, root=ROOT):
        """Whole campaigns absorb their parts; a large part becomes the whole."""
        corpus = len(corpus_cases('conformance/', root)) or 1
        for campaign, cases in ((impact.CONFORMANCE, self.conformance), (impact.AGREEMENT, self.agreement)):
            if len(cases) > WHOLE_CORPUS_SHARE * corpus:
                self.campaigns.add(campaign)
            if campaign in self.campaigns:
                cases.clear()
        self.agreement = {case for case in self.agreement if accepted(case, root)}

    def config(self) -> list[str]:
        """The Buck2 settings carrying the chosen cases and native programs
        (defs.bzl selected_paths and native_filters): long, so gating.buck2
        passes them through an argfile."""
        settings = []
        if self.conformance:
            settings.append('hirsel.conformance_paths=' + ';'.join(
                '|'.join(part) for part in slices(sorted(self.conformance), CONFORMANCE_SLICE_PROGRAMS)))
        if self.agreement:
            settings.append('hirsel.agree_paths=' + ';'.join(
                '|'.join(part) for part in slices(sorted(self.agreement), AGREE_SLICE_PROGRAMS)))
        if self.native:
            settings.append('hirsel.native_filters=' + '|'.join(sorted(self.native)))
        return [argument for setting in settings for argument in ('-c', setting)]

    def campaign_targets(self) -> list[str]:
        targets = [t for campaign in impact.CAMPAIGNS if campaign in self.campaigns for t in CAMPAIGN_TARGETS[campaign]]
        targets += [f'root//:gate-conformance-selected-shard-{i}' for i in
                    range(1, len(slices(sorted(self.conformance), CONFORMANCE_SLICE_PROGRAMS)) + 1)]
        targets += [f'root//:gate-agree-selected-shard-{i}' for i in
                    range(1, len(slices(sorted(self.agreement), AGREE_SLICE_PROGRAMS)) + 1)]
        if self.native:
            targets += NATIVE_TARGETS
        return targets

    def slot_pool(self) -> str:
        """Classify the settled, combined floor and choice before queueing."""
        if self.campaigns or self.native:
            return NORMAL_POOL
        targets = self.tests | set(self.campaign_targets())
        return EXPRESS_POOL if len(targets) <= EXPRESS_TEST_TARGETS else NORMAL_POOL

    def summary(self) -> list[str]:
        """The plan in a line per part, for the green record and tools/land."""
        binaries = {}
        for target in self.tests:
            name = short(target)
            binaries.setdefault(SHARD.sub(r'\1', name), set()).add(name)
        lines = sorted(name if len(shards) == 1 and name in shards else f'{name} ({len(shards)} shards)'
                       for name, shards in binaries.items())
        lines += sorted(name for name, campaign in WHOLE.items() if campaign in self.campaigns)
        for prefix, cases in ((CONFORMANCE_PREFIX, self.conformance), (AGREEMENT_PREFIX, self.agreement),
                              (NATIVE_PREFIX, self.native)):
            if cases:
                shown = sorted(cases)
                lines.append(f'{prefix}{" ".join(shown[:6])}' + (f' and {len(shown) - 6} more' if len(shown) > 6 else ''))
        return lines


NATIVE_TARGETS = ['root//:gate-native-changed', 'root//:gate-native-cranelift-changed']


class Selection:
    """What a change must run (the floor), what the gate suggests for it, and
    what a lane chose."""

    def __init__(self, base, root=ROOT, paths=None):
        self.base = base
        self.root = root
        self.paths = changed_paths(base, root) if paths is None else paths
        buck_text = (root / 'BUCK').read_text()
        self.graph = graph.Graph(buck_text, root)
        self.classified = impact.classify(self.paths, members(root), buck_text)
        if 'BUCK' in self.paths:
            old = subprocess.run(['git', 'show', f'{base}:BUCK'], cwd=root, capture_output=True, text=True).stdout
            self.classified.buck = impact.buck_targets(old, buck_text)
        if 'tools/buck2/test_sizes.bzl' in self.paths:
            old = git('show', f'{base}:tools/buck2/test_sizes.bzl', root=root)
            self.classified.test_requests = impact.changed_test_requests(
                old, (root / 'tools/buck2/test_sizes.bzl').read_text())

    @property
    def crates(self):
        return self.classified.crates

    def owner_tests(self, paths) -> set[str]:
        """The tests reading these files, including through their harnesses."""
        files = owned_paths(paths, self.root)
        return labels(self.graph.tests(self.graph.rdeps(self.graph.owners(files))))

    def ratchet(self) -> set[str]:
        if impact.PASSING not in self.paths:
            return set()
        return impact.ratchet_programs(subprocess.run(
            ['git', 'diff', '-U0', self.base, '--', impact.PASSING], cwd=self.root,
            capture_output=True, text=True).stdout)

    def floor(self) -> Plan:
        """What every gate runs: the test targets of each crate whose source
        changed and of each changed test file, and every changed or added
        corpus case with its twins (the workspace build, Clippy and formatting
        run before it)."""
        plan = Plan()
        c = self.classified
        if c.crates:
            plan.tests |= labels(self.graph.crate_tests(c.crates))
            plan.reasons.append((', '.join(sorted(c.crates)), 'the crates whose source changed: all their tests'))
        tests = [p for p in c.owned if p.startswith('crates/') and p.split('/')[1] not in c.crates]
        if tests:
            if owners := self.owner_tests(tests):
                plan.tests |= owners
                plan.reasons.append(('test code', 'the test targets of the changed test files'))
        cases = impact.changed_programs(self.paths, self.root, self.ratchet())
        if cases:
            plan.conformance |= set(cases)
            plan.reasons.append((CONFORMANCE_PREFIX + '<changed>', f'{len(cases)} changed or added corpus cases and their twins'))
        if dependents := self.std_dependents():
            # std is source too: its tests are the cases that import the changed modules.
            plan.conformance |= set(dependents)
            plan.reasons.append((CONFORMANCE_PREFIX + '<std>', self.std_why + ': ' + ', '.join(dependents)))
        if inputs := self.std_test_inputs():
            plan.tests |= self.owner_tests(inputs)
            plan.reasons.append(('<std package tests>',
                                 'package and example import closures reach the changed std modules: '
                                 + ', '.join(inputs)))
        return plan

    def std_test_inputs(self) -> list[str]:
        """Package and example inputs whose import closure reaches changed std."""
        if not hasattr(self, '_std_test_inputs'):
            self._std_test_inputs = []
            if any(p.startswith(impact.STD + '/') for p in self.paths):
                versions, changed = self.std_change()
                self._std_test_inputs = sorted({path for std in versions
                                               for path in impact.std_test_inputs(changed, self.root, std)})
        return self._std_test_inputs

    def std_examples(self) -> list[str]:
        """The examples whose import closure includes a changed std module."""
        if not any(p.startswith(impact.STD + '/') for p in self.paths):
            return []
        versions, changed = self.std_change()
        examples = set()
        for std in versions:
            reach = std.reach()
            for manifest in self.root.glob('examples/*/hirsel.json'):
                found = impact.package_std_reach(manifest, self.root, std, reach)
                if found and (changed is None or found & changed):
                    examples.add(manifest.parent.name)
        return sorted(examples)

    def std_change(self):
        """The current and, for a manifest edit, base import graphs and changed modules.

        Retain the base graph so removing a source still selects its former
        dependents, including transitive imports and shared core/std heads.
        """
        if not hasattr(self, '_std_change'):
            std = impact.Std(self.root)
            previous = None
            if impact.STD_MANIFEST in self.paths:
                old = subprocess.run(['git', 'show', f'{self.base}:{impact.STD_MANIFEST}'],
                                     cwd=self.root, capture_output=True, text=True)
                if old.returncode == 0:
                    previous = impact.Std(self.root, read_text=lambda path: git(
                        'show', f'{self.base}:{path}', root=self.root))
            versions = [std, previous] if previous is not None else [std]
            self._std_change = versions, impact.changed_std_modules(self.paths, std, previous)
        return self._std_change

    def std_dependents(self) -> list[str]:
        """The corpus cases whose import closure includes a changed std module."""
        if not hasattr(self, '_std'):
            self._std, self.std_why = [], ''
            if any(p.startswith(impact.STD + '/') for p in self.paths):
                versions, changed = self.std_change()
                self._std = sorted({case for std in versions
                                    for case in impact.std_dependents(changed, self.root, std)})
                named = 'all of std' if changed is None else 'std ' + ', '.join(sorted('.'.join(m) for m in changed))
                self.std_why = f'{len(self._std)} corpus cases whose import closure includes {named}'
        return self._std

    def suggest(self) -> Plan:
        """What the change can affect (impact.py), as a lane's starting point."""
        plan = Plan()
        c = self.classified
        root = self.root
        if c.everything:
            plan.tests |= labels(self.graph.all_tests())
            plan.campaigns |= set(impact.CAMPAIGNS)
            plan.reasons.append((f'{ALL} conformance agreement native-smoke',
                                 'a build-wide input, an unknown crate or the BUCK header changed'))
            return plan
        scope = set()
        data = [p for p in c.owned if not p.startswith('crates/')]
        if c.owned:
            scope |= self.graph.owners(owned_paths(c.owned, root))
        if isinstance(c.buck, set) and c.buck:
            scope |= self.graph.macro_targets(c.buck)
        if scope:
            plan.tests |= labels(self.graph.tests(self.graph.rdeps(scope)))
            plan.reasons.append(('<tests>', 'every Buck2 test reading a changed file, directly or through a dependency'))
        if c.crates:
            plan.campaigns |= c.campaigns
            if c.campaigns:
                plan.reasons.append((' '.join(n for n, k in WHOLE.items() if k in c.campaigns),
                                     'library source of ' + ', '.join(sorted(c.crates)) + ' (impact.CRATE_CAMPAIGNS)'))
        if native := impact.native_programs(self.paths, c.crates, root):
            plan.native |= set(native)
            plan.reasons.append((NATIVE_PREFIX + '<programs>', 'a native producer changed: its programs on both backends'))
        cases = impact.changed_programs(self.paths, root, self.ratchet())
        if cases:
            plan.conformance |= set(cases)
            plan.agreement |= set(cases)
            plan.reasons.append((f'{CONFORMANCE_PREFIX}<changed> {AGREEMENT_PREFIX}<changed>',
                                 'changed corpus cases and their twins, and their variants in the seed-1 sample'))
        if dependents := self.std_dependents():
            plan.agreement |= set(dependents)
            plan.reasons.append((f'{AGREEMENT_PREFIX}<std>', f'the seed-1 sample\'s variants of the {self.std_why} '
                                 '(a variant mutates its own case, which imports std)'))
        if dependents := impact.package_dependents(self.paths, root):
            plan.conformance |= set(dependents)
            plan.agreement |= set(dependents)
            plan.reasons.append((f'{CONFORMANCE_PREFIX}<packages>',
                                 f'{len(dependents)} corpus cases whose packages reach a changed file'))
        for path in data:
            if campaigns := impact.campaign_data(path):
                plan.campaigns |= campaigns
                plan.reasons.append((' '.join(n for n, k in WHOLE.items() if k in campaigns),
                                     f'{path} is data every case of the campaign can read'))
        if c.test_requests:
            # A reservation changes the run, not its harness or downstream users.
            plan.tests |= labels(name for name in self.graph.macro_targets(c.test_requests)
                                 if self.graph.targets[name].test)
            for campaign, targets in CAMPAIGN_TARGETS.items():
                if any(re.sub(r'-shard-\d+$', '', short(t)) in c.test_requests for t in targets):
                    plan.campaigns.add(campaign)
            plan.reasons.append(('<resized>', 'the binaries whose pool request changed'))
        if not any(p.startswith(EXAMPLES_INPUTS) for p in self.paths):
            plan.tests = {t for t in plan.tests if not short(t).startswith(EXAMPLES + '-shard-')
                          and short(t) != EXAMPLES}
        plan.add(self.floor())
        return plan

    def choose(self, tokens) -> Plan:
        """A lane's `--tests`: tokens as `tools/gate --help` lists them."""
        plan = Plan()
        names = None
        for token in tokens:
            if token == SUGGESTED:
                plan.add(self.suggest())
            elif token == ALL:
                plan.tests |= labels(self.graph.all_tests())
                plan.reasons.append((ALL, 'chosen'))
            elif token in WHOLE:
                plan.campaigns.add(WHOLE[token])
                plan.reasons.append((token, 'chosen'))
            elif token.startswith((CONFORMANCE_PREFIX, AGREEMENT_PREFIX)):
                prefix, _, path = token.partition(':')
                cases = corpus_cases(path, self.root)
                if not cases:
                    raise SystemExit(f'gate: --tests {token}: no corpus case starts with {path!r}')
                (plan.conformance if prefix == 'conformance' else plan.agreement).update(cases)
                plan.reasons.append((token, 'chosen'))
            elif token.startswith(NATIVE_PREFIX):
                program = token.removeprefix(NATIVE_PREFIX)
                valid = (generated_program_id(program) if program.startswith('generated/')
                         else (self.root / program).is_file())
                if not valid:
                    raise SystemExit(f'gate: --tests {token}: {program} is neither a file nor a generated program ID')
                plan.native.add(program)
                plan.reasons.append((token, 'chosen'))
            else:
                if names is None:
                    names = self.graph.all_tests()
                name = short(token.removeprefix('//:'))
                shards = sorted(n for n in names if re.fullmatch(re.escape(name) + r'-shard-\d+', n))
                if name not in names and not shards:
                    raise SystemExit(f'gate: --tests {token}: no such test target (tools/gate --help lists the forms)')
                plan.tests |= {f'root//:{n}' for n in (shards or [name])}
                plan.reasons.append((token, 'chosen'))
        return plan


# ---------------------------------------------------------------- results

# Buck2's TestStatus (buck2_data.proto).
PASS, FAIL, SKIP, OMITTED, FATAL, TIMED_OUT = 1, 2, 3, 4, 5, 6
STATUS = {PASS: 'pass', FAIL: 'FAIL', SKIP: 'skip', OMITTED: 'omitted', FATAL: 'FATAL', TIMED_OUT: 'TIMEOUT'}


def test_results(event_log, *, history=None) -> dict[str, dict]:
    """Each test's TestResult event from a `buck2 test` event log, by short name."""
    events = subprocess.run([str(BUCK2), 'log', 'show', str(event_log)], cwd=ROOT, check=True,
                            stdout=subprocess.PIPE, text=True, errors='replace').stdout
    results, rows = {}, []
    for line in events.splitlines():
        if '"TestResult"' not in line:
            continue
        event = json.loads(line).get('Event', {})
        data = event.get('data', {}).get('Instant', {}).get('data', {})
        if result := data.get('TestResult'):
            stamp = event.get('timestamp')
            date = (datetime.fromtimestamp(stamp[0] + stamp[1] / 1e9, timezone.utc) if stamp
                    else datetime.now(timezone.utc)).isoformat()
            name = short(result['name'])
            seconds = (result.get('duration_us') or 0) / 1e6
            results[name] = dict(status=result.get('status'), details=result.get('details') or '', seconds=seconds)
            match = re.fullmatch(r'(.+)-shard-(\d+)', name)
            costs = event_logs.case_costs(result.get('details') or '')
            rows.append(dict(gate=Path(event_log).name.removesuffix('.pb.zst'),
                             target=match[1] if match else name, shard=match[2] if match else '',
                             duration_seconds=seconds, date=date, status=result.get('status'),
                             case_costs=json.dumps(costs, sort_keys=True, separators=(',', ':')) if costs else ''))
    (event_logs.test_history() if history is None else history).append(rows)
    return results


def short(target):
    return target.removeprefix('root//:').split(' ')[0]


SHARD = re.compile(r'^(.+)-shard-\d+$')


def failed_cases(details):
    cases = re.findall(r'^test (\S+)(?: - should panic)? \.\.\. FAILED$', details, re.M)
    cases += re.findall(r'^---- (\S+) stdout ----$', details, re.M)
    return list(dict.fromkeys(cases))


# ---------------------------------------------------------------- known failures

# Failures main is known to have, each owned by the lane fixing it: a gate
# excludes them and prints every exclusion. Rows are added to main directly
# (tools/land refuses a change that adds one) and expire when their owner lane
# lands (tools/land removes them).
KNOWN_FAILURES = 'tools/known-failures.tsv'
KNOWN_COLUMNS = ('id', 'reason', 'owner', 'date')


@dataclass(frozen=True)
class KnownFailure:
    """`id` is a test target (`hirsel-rt-test-it-shard-2`), a binary and case
    (`hirsel-rt-test-it::replay::kill_order`) or a corpus case
    (`conformance/MEM/MEM-7-x-run.hir`, in conformance or agreement)."""
    id: str
    reason: str
    owner: str
    date: str

    def describe(self):
        return f'known failure {self.id}: {self.reason} (owner {self.owner}, since {self.date})'


def parse_known_failures(text, name=KNOWN_FAILURES) -> dict[str, KnownFailure]:
    rows = {}
    for number, line in enumerate(text.splitlines(), 1):
        if not line.strip() or line.startswith('#'):
            continue
        fields = [field.strip() for field in line.split('\t')]
        if len(fields) != len(KNOWN_COLUMNS) or not all(fields):
            raise SystemExit(f'gate: {name}:{number}: a row is {", ".join(KNOWN_COLUMNS)}, separated by tabs')
        rows[fields[0]] = KnownFailure(*fields)
    return rows


def known_failures(root=ROOT) -> dict[str, KnownFailure]:
    """origin/main's list: a lane can only remove rows, so main's is in effect."""
    shown = subprocess.run(['git', 'show', f'origin/main:{KNOWN_FAILURES}'], cwd=root, capture_output=True, text=True)
    return parse_known_failures(shown.stdout) if shown.returncode == 0 else {}


def failing_ids(target, details) -> list[str]:
    """The ids of what failed in a target: libtest cases as `binary::case`,
    corpus cases by path (conformance lines and agreement clusters)."""
    binary = SHARD.sub(r'\1', target)
    ids = [f'{binary}::{case}' for case in failed_cases(details)]
    ids += re.findall(r'^(conformance/\S+\.hir): ', details, re.M)
    ids += re.findall(r'"path"\s*:\s*"(conformance/[^"]+\.hir)"', details)
    return list(dict.fromkeys(ids))


def known_only(target, details, known) -> list[KnownFailure] | None:
    """The known failures that explain a failed target, or None when something
    else failed (or nothing names what did)."""
    for name in (target, SHARD.sub(r'\1', target)):
        if name in known:
            return [known[name]]
    ids = failing_ids(target, details)
    if ids and all(i in known for i in ids):
        return [known[i] for i in ids]
    return None


def added_known_failures(diff) -> list[str]:
    """Rows a `git diff` of the known-failures list adds."""
    return [line[1:] for line in diff.splitlines()
            if line.startswith('+') and not line.startswith('+++') and line[1:].strip()
            and not line.startswith('+#')]


def without_owners(text, owners) -> str:
    """The list without the rows these lanes own."""
    def kept(line):
        fields = line.split('\t')
        return line.startswith('#') or len(fields) < 3 or fields[2].strip() not in owners
    return ''.join(line for line in text.splitlines(keepends=True) if kept(line))


def pool_error(details):
    """The remote execution error of a test the pool never ran (for example one
    that queued past its timeout), or None."""
    match = re.search(r'^(?:Remote Execution Error.*|Error: \(.*(?:remote execution|returned on the stream by RE).*)$',
                      details, re.M | re.I)
    return match[0] if match else None


def rerun(target, details) -> list[str]:
    """What failed in one test target, and the commands that rerun it alone."""
    buck = '.buck2/bin/buck2 test'
    if error := pool_error(details):
        return [f'the pool never ran it: {error[:300]}', f'{buck} //:{target}']
    if target.startswith('gate-conformance-'):
        lines = []
        for line, path in re.findall(r'^((conformance/\S+\.hir): .*)$', details, re.M):
            lines += [line, f'{buck} -c hirsel.tests=focus //:gate-conformance-shard-1 -- --test-arg=--shard '
                            f'--test-arg=1/1 --test-arg=--path --test-arg={path}']
        return lines + [f'{buck} //:{target}']
    if target.startswith('gate-agree-'):
        lines = []
        for match in re.finditer(r'(?m)^\{', details):
            try:
                report, _ = json.JSONDecoder().raw_decode(details[match.start():])
            except ValueError:
                continue
            for cluster in report.get('clusters', []) if isinstance(report, dict) else []:
                path = cluster.get('path', '?')
                lines.append(f'{cluster.get("kind", "discrepancy")}: {path}')
                # The gate's own sample, restricted to the case: the same variants
                # (--program would replay the unmutated case).
                lines.append('.buck2/bin/buck2 run --target-platforms //tools/buck2:release '
                             f'//:hirsel-conformance-bin-agree -- --root "$PWD" --seed 1 --limit {AGREE_LIMIT} '
                             f'--sample-path {path}')
        return lines + [f'{buck} //:{target}']
    if target == 'gate-native-smoke':
        lines = re.findall(r'^((?:smoke|real Io replay): .*)$', details, re.M)
        return lines + [f'{buck} //:{target}']
    cases = failed_cases(details)
    if not cases:
        return [f'{buck} //:{target}']
    # A filtered run of a sharded binary executes in its first shard.
    one = (m[1] + '-shard-1') if (m := SHARD.match(target)) else target
    return [f'{case}\n    {buck} //:{one} -- --test-arg={shlex.quote(case)} --test-arg=--exact' for case in cases]


def build_failures(report_path) -> dict[str, str]:
    """Targets that failed to build, with their first error, from a Buck2 build report."""
    try:
        report = json.loads(Path(report_path).read_text())
    except (OSError, ValueError):
        return {}
    failures = {}
    for target, result in report.get('results', {}).items():
        if result.get('success') == 'FAIL':
            errors = [e.get('message_content') or e.get('message') or '' for c in result.get('configured', {}).values()
                      for e in c.get('errors', [])] or [str(result.get('errors', ''))]
            failures[short(target)] = next((e for e in errors if e), 'build failed')
    return failures


def last_lines(text, count=12):
    lines = [line for line in text.splitlines() if line.strip()]
    return '\n'.join(lines[-count:])


# ---------------------------------------------------------------- local stages

class Local:
    """Commands on this host that run beside the Buck2 invocation."""

    def __init__(self):
        self.pool = ThreadPoolExecutor(max_workers=8)
        self.jobs = []

    def start(self, name, command, cwd=ROOT):
        started = time.monotonic()

        def job():
            result = run(command, cwd=cwd, capture_output=True, text=True)
            return result.returncode, result.stdout + result.stderr, time.monotonic() - started
        self.jobs.append((name, command, self.pool.submit(job)))

    def finish(self):
        results = [(name, command, *future.result()) for name, command, future in self.jobs]
        self.pool.shutdown()
        return results


def tool_tests():
    """The gate tooling's own tests and the native benchmark harness's, each its own command."""
    tests = [(f'tools/tests/{p.name}', [sys.executable, str(p)]) for p in sorted((ROOT / 'tools/tests').glob('*.py'))]
    return tests + [('conformance.py --self-test', [sys.executable, str(ROOT / 'tools/buck2/conformance.py'), '--self-test']),
                    ('bench/native/tests', [sys.executable, '-m', 'unittest', 'discover', '-s',
                                            str(ROOT / 'bench/native/tests')])]


def clippy_report(targets) -> tuple[bool, str]:
    """Clippy's diagnostics for the twins the invocation built; fetching the
    small `[clippy.txt]` outputs is a cache hit."""
    import clippy
    for attempt in (1, 2):
        report = buck2('build', '--show-full-json-output', *[t + '[clippy.txt]' for t in targets],
                       capture_output=True, text=True)
        # The pool may have dropped an output the invocation produced minutes ago
        # (its action result's ttl ran out); Buck2 then reruns that action on the
        # next build.
        if report.returncode == 0 or 'Error materializing' not in report.stderr:
            break
    if report.returncode:
        return False, report.stderr
    failed, text = clippy.diagnostics(json.loads(report.stdout or '{}'))
    return not failed, text + ('clippy: errors in ' + ', '.join(failed) + '\n' if failed else '')
