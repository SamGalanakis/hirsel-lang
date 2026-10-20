"""What tools/gate, tools/land and tools/main-runs share: a change's paths,
the selection of what it can affect (impact.py), one Buck2 invocation for all
of it, the land queue's lock, known failures and the failure summary."""

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
import generated
import graph
from sync import AGREE_LIMIT, AGREE_SLICE_PROGRAMS, CONFORMANCE_SLICE_PROGRAMS, SELECTED_SLICES
from test_sizes_from_usage import requests_bzl

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
             ' - attrfilter(labels, gate, root//:)'
             # The two-host link build runs under one host-wide lock; it is part of
             # the native-smoke campaign (gate-link-hosts depends on it), never land's build.
             ' - kind("_link_hosts_local", root//:)')
CLIPPY_SET = 'attrregexfilter(name, "-clippy$", kind("^rust_(library|binary)$", root//:))'
# The one whole campaign a lane gate runs: the whole conformance and agreement
# campaigns run hourly on main (tools/main-runs, `tools/gate --full`).
CAMPAIGN_TARGETS = {
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

def process_start(pid):
    """Linux process identity survives PID reuse; zombies cannot keep a ticket.
    The land queue (tools/land) keeps its waiters' tickets and its lock this way."""
    try:
        fields = Path(f'/proc/{pid}/stat').read_text().rsplit(')', 1)[1].split()
    except (FileNotFoundError, ProcessLookupError):
        return None
    return None if fields[0] == 'Z' else int(fields[19])


def live_tickets(queue):
    """The queue's live waiters, earliest first, as (stamp, pid, ticket);
    tickets left by exited lands are discarded."""
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


def oldest_ticket(queue):
    """The earliest live waiter's ticket."""
    live = live_tickets(queue)
    return live[0][2] if live else None


def process_cwd(pid):
    """A process's working directory: a land's worktree."""
    try:
        return os.readlink(f'/proc/{pid}/cwd')
    except OSError:
        return '?'


def holder(path):
    """The land lock's live holder as (pid, worktree, acquired), or None when free."""
    try:
        fields = path.read_text().split()
        acquired = path.stat().st_mtime
    except (OSError, ValueError):
        return None
    if len(fields) < 2 or not fields[0].isdigit() or str(process_start(int(fields[0]))) != fields[1]:
        return None
    # A live process may have released the lock: probe it without waiting.
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


def queue_status(directory, *, me=(), now=None):
    """The land lock's holder with its age and the queue in order, as lines.
    `me` are this lane's worktrees: their holder and waiters are marked."""
    now = time.time() if now is None else now
    found = holder(directory / 'slot-0')
    waiting = [(stamp, pid, process_cwd(pid)) for stamp, pid, _ in live_tickets(directory / 'queue')]
    lines = [f'land: land lock: 1 in {directory}; {int(bool(found))} held, {len(waiting)} queued']
    if found:
        pid, worktree, acquired = found
        lines.append(f'  held {minutes(now - acquired)} by {worktree} (pid {pid})' + (' (you)' if worktree in me else ''))
    for position, (stamp, pid, worktree) in enumerate(waiting, 1):
        mine = ' (you)' if worktree in me else ''
        lines.append(f'  queued {position}: waiting {minutes(now - stamp / 1e9)}, {worktree} (pid {pid}){mine}')
    if found and found[1] in me:
        lines.append('land: you hold the land lock')
    positions = [position for position, (_, _, worktree) in enumerate(waiting, 1) if worktree in me]
    if positions:
        lines.append(f'land: you are at {positions[0]} of {len(waiting)} in the land queue')
    return lines


@dataclass
class SlotTiming:
    wait_seconds: float
    held_seconds: float = 0


# ---------------------------------------------------------------- git

def git(*arguments, root=ROOT, **kwargs) -> str:
    return subprocess.run(['git', *arguments], cwd=root, check=True, capture_output=True,
                          text=True, **kwargs).stdout


def merge_base(base, root=ROOT) -> str:
    return git('merge-base', 'HEAD', base, root=root).strip()


def changed_paths(base, root=ROOT, deleted=True) -> list[str]:
    """Paths the worktree changes from `base` (a commit), committed or not,
    tracked or untracked, and unless `deleted` is false, the paths it deletes.
    Main may advance while a gate runs, so `base` is the lane's merge base,
    never main itself."""
    paths = []
    for arguments in (['diff', '--no-renames', '--name-only', '-z', *([] if deleted else ['--diff-filter=d']), base],
                      ['ls-files', '--others', '--exclude-standard', '-z']):
        paths.extend(os.fsdecode(path) for path in subprocess.check_output(
            ['git', *arguments], cwd=root).split(b'\0') if path)
    return sorted(set(paths))


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
WHOLE = {'native-smoke': impact.NATIVE_SMOKE}
# The corpus smoke slice (impact.smoke_slice) with its agreement variants.
SMOKE = 'smoke'
# Whole campaigns no lane gate runs: they run hourly on main.
MAIN_ONLY = ('conformance', 'agreement')
CONFORMANCE_PREFIX, AGREEMENT_PREFIX, NATIVE_PREFIX = 'conformance:', 'agreement:', 'native:'
# The CLI's end-to-end examples: each builds and runs a program on both
# backends, so they run when the CLI, the driver or an example changes.
EXAMPLES = 'hirsel-cli-test-examples'
EXAMPLES_INPUTS = ('crates/hirsel-cli/', 'crates/hirsel-driver/', 'examples/')


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
    """What one gate runs: Buck2 tests, the native smoke campaign, the corpus
    smoke slice, chosen corpus cases for conformance and for agreement's
    seed-1 sample, and native programs. No plan holds a whole conformance or
    agreement campaign: they run hourly on main."""
    tests: set = field(default_factory=set)
    campaigns: set = field(default_factory=set)
    conformance: set = field(default_factory=set)
    agreement: set = field(default_factory=set)
    native: set = field(default_factory=set)
    smoke: bool = False
    # (token, why) for every part, in the order they were chosen.
    reasons: list = field(default_factory=list)
    # The smoke slice's cases, read from the corpus by settle().
    smoke_cases: set = field(default_factory=set)
    smoke_agreement: set = field(default_factory=set)

    def add(self, other: 'Plan'):
        self.tests |= other.tests
        self.campaigns |= other.campaigns
        self.conformance |= other.conformance
        self.agreement |= other.agreement
        self.native |= other.native
        self.smoke |= other.smoke
        self.reasons += [r for r in other.reasons if r not in self.reasons]

    def settle(self, root=ROOT):
        """Read the smoke slice from the corpus; agreement keeps the cases it mutates."""
        self.smoke_cases = set(impact.smoke_slice(root)) if self.smoke else set()
        self.smoke_agreement = {case for case in self.smoke_cases if accepted(case, root)}
        self.agreement = {case for case in self.agreement if accepted(case, root)}

    def corpus(self) -> tuple[list[str], list[str]]:
        """The conformance cases and the agreement cases the plan runs: the
        chosen ones and the smoke slice's."""
        return sorted(self.conformance | self.smoke_cases), sorted(self.agreement | self.smoke_agreement)

    def config(self) -> list[str]:
        """The Buck2 settings carrying the corpus cases and native programs
        (defs.bzl selected_paths and native_filters): long, so gating.buck2
        passes them through an argfile."""
        settings = []
        conformance, agreement = self.corpus()
        if conformance:
            settings.append('hirsel.conformance_paths=' + ';'.join(
                '|'.join(part) for part in slices(conformance, CONFORMANCE_SLICE_PROGRAMS)))
        if agreement:
            settings.append('hirsel.agree_paths=' + ';'.join(
                '|'.join(part) for part in slices(agreement, AGREE_SLICE_PROGRAMS)))
        if self.native:
            settings.append('hirsel.native_filters=' + '|'.join(sorted(self.native)))
        return [argument for setting in settings for argument in ('-c', setting)]

    def campaign_targets(self) -> list[str]:
        conformance, agreement = self.corpus()
        targets = [t for campaign in impact.CAMPAIGNS if campaign in self.campaigns for t in CAMPAIGN_TARGETS[campaign]]
        targets += [f'root//:gate-conformance-selected-shard-{i}' for i in
                    range(1, len(slices(conformance, CONFORMANCE_SLICE_PROGRAMS)) + 1)]
        targets += [f'root//:gate-agree-selected-shard-{i}' for i in
                    range(1, len(slices(agreement, AGREE_SLICE_PROGRAMS)) + 1)]
        if self.native:
            targets += NATIVE_TARGETS
        return targets


def ensure_build_files(root=ROOT):
    """Regenerate the gitignored build files (generated.py) when the tree
    changed since they were generated."""
    synced = generated.ensure(root)
    if synced is not None and synced.returncode:
        raise SystemExit('cannot generate the Buck2 build files (tools/buck2/sync.py)\n'
                         + last_lines(synced.stdout + synced.stderr, 20))


# Root BUCK files of base commits, by commit, in the common git directory.
BASE_BUCKS = 'hirsel-base-buck'
KEEP_BASE_BUCKS = 64


def base_buck(base, root=ROOT) -> str:
    """The root BUCK of commit `base`, which a change's BUCK is compared with by
    target: generated by the base's own sync.py in a scratch copy of it (once
    per commit, kept in the common git directory), or as committed before the
    build files were gitignored."""
    committed = subprocess.run(['git', 'show', f'{base}:BUCK'], cwd=root, capture_output=True, text=True)
    if committed.returncode == 0:
        return committed.stdout
    copies = Path(git('rev-parse', '--path-format=absolute', '--git-common-dir', root=root).strip()) / BASE_BUCKS
    cached = copies / base
    if cached.is_file():
        return cached.read_text()
    copies.mkdir(exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='hirsel-base-buck-') as scratch:
        tree = Path(scratch) / 'tree'
        tree.mkdir()
        archive = subprocess.Popen(['git', 'archive', base], cwd=root, stdout=subprocess.PIPE)
        extracted = subprocess.run(['tar', '-x', '-C', str(tree)], stdin=archive.stdout)
        archive.stdout.close()
        if archive.wait() or extracted.returncode:
            raise SystemExit(f'cannot extract {base[:12]} to generate its root BUCK')
        made = subprocess.run([sys.executable, str(tree / 'tools/buck2/sync.py'), '--root-buck', str(Path(scratch) / 'BUCK')],
                              cwd=tree, capture_output=True, text=True, env=os.environ | {'HIRSEL_LAND_TREE': str(tree)})
        if made.returncode:
            raise SystemExit(f'cannot generate the root BUCK of {base[:12]} to compare this change\'s with\n'
                             + last_lines(made.stdout + made.stderr, 20))
        text = (Path(scratch) / 'BUCK').read_text()
    pending = copies / f'.{base}.{os.getpid()}'
    pending.write_text(text)
    pending.replace(cached)
    for old in sorted((p for p in copies.iterdir() if not p.name.startswith('.')),
                      key=lambda p: p.stat().st_mtime)[:-KEEP_BASE_BUCKS]:
        old.unlink(missing_ok=True)
    return text


def ensure_build_files_for(paths, root=ROOT) -> bool:
    """Regenerate stale build files unless the changed paths are documentation
    alone, which changes none: whether they are."""
    documentation = all(impact.matches(path, impact.DOCS) for path in paths)
    if not documentation or not (root / 'BUCK').is_file():
        ensure_build_files(root)
    return documentation


NATIVE_TARGETS = ['root//:gate-native-changed', 'root//:gate-native-cranelift-changed']


class Selection:
    """What a change can affect (the targeted set) and what a lane chose."""

    def __init__(self, base, root=ROOT, paths=None):
        self.base = base
        self.root = root
        self.paths = changed_paths(base, root) if paths is None else paths
        documentation = ensure_build_files_for(self.paths, root)
        buck_text = (root / 'BUCK').read_text()
        self.graph = graph.Graph(buck_text, root)
        self.classified = impact.classify(self.paths, members(root), buck_text)
        if not documentation and (old := base_buck(base, root)) != buck_text:
            self.classified.buck = impact.buck_targets(old, buck_text)
        if impact.TEST_SIZES in self.paths:
            self.classified.test_requests = impact.changed_test_requests(
                requests_bzl(json.loads(git('show', f'{base}:{impact.TEST_SIZES}', root=root))),
                requests_bzl(json.loads((root / impact.TEST_SIZES).read_text())))

    @property
    def crates(self):
        return self.classified.crates

    def compiler_crates(self) -> set[str]:
        """The changed crates the corpus campaigns run through (impact.CRATE_CAMPAIGNS)."""
        return {crate for crate in self.crates
                if impact.CRATE_CAMPAIGNS.get(crate, impact.ALL) & impact.FRONT} or set(self.crates)

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

    def core(self) -> Plan:
        """The targeted set's own tests: the test targets of each crate whose
        source changed and of each changed test file, the smoke slice for a
        compiler crate, every changed or added corpus case with its twins, and
        the cases and package tests importing changed std."""
        plan = Plan()
        c = self.classified
        if c.crates:
            plan.tests |= labels(self.graph.crate_tests(c.crates))
            plan.reasons.append((', '.join(sorted(c.crates)), 'the crates whose source changed: all their tests'))
        if c.campaigns & impact.FRONT:
            # No lane runs a whole conformance or agreement campaign (main runs them hourly).
            plan.smoke = True
            plan.reasons.append((SMOKE, 'compiler crate source of ' + ', '.join(sorted(self.compiler_crates()))
                                 + ': one corpus case per rule and expectation kind, and its agreement variants'))
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
        """The targeted set: what the change can affect (impact.py). A crate's
        library source runs its own tests and the smoke slices, never every
        test that depends on the crate: main's hourly run is the whole net."""
        plan = Plan()
        c = self.classified
        root = self.root
        if c.everything:
            plan.tests |= labels(self.graph.all_tests())
            plan.campaigns.add(impact.NATIVE_SMOKE)
            plan.smoke = True
            plan.reasons.append((f'{ALL} {SMOKE} native-smoke',
                                 'a build-wide input, an unknown crate or the BUCK header changed'))
            plan.add(self.core())
            return plan
        if impact.perf_counts(self.paths):
            plan.tests.add(impact.PERF_COUNTS)
            plan.reasons.append(('gate-perf-counts', 'deterministic native, micro and compiler instruction budgets'))
        scope = set()
        data = [p for p in c.owned if not p.startswith('crates/')]
        if read := [p for p in c.owned if not impact.library_source(p)]:
            scope |= self.graph.owners(owned_paths(read, root))
        if isinstance(c.buck, set) and c.buck:
            scope |= self.graph.macro_targets(c.buck)
        if scope:
            plan.tests |= labels(self.graph.tests(self.graph.rdeps(scope)))
            plan.reasons.append(('<tests>', 'every Buck2 test reading a changed file, directly or through a dependency'))
        if impact.NATIVE_SMOKE in c.campaigns:
            plan.campaigns.add(impact.NATIVE_SMOKE)
            plan.reasons.append(('native-smoke', 'library source of ' + ', '.join(sorted(c.crates))
                                 + ' (impact.CRATE_CAMPAIGNS)'))
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
                plan.campaigns |= campaigns & {impact.NATIVE_SMOKE}
                plan.smoke |= bool(campaigns & impact.FRONT)
                plan.reasons.append((' '.join([SMOKE] * bool(campaigns & impact.FRONT)
                                              + ['native-smoke'] * (impact.NATIVE_SMOKE in campaigns)),
                                     f'{path} is data every case of the campaign can read'))
        if c.test_requests:
            # A reservation changes the run, not its harness or downstream users.
            plan.tests |= labels(name for name in self.graph.macro_targets(c.test_requests)
                                 if self.graph.targets[name].test)
            for campaign, targets in CAMPAIGN_TARGETS.items():
                if any(re.sub(r'-shard-\d+$', '', short(t)) in c.test_requests for t in targets):
                    plan.campaigns.add(campaign)
            # The corpus slices are measured as their whole campaigns' binaries.
            plan.smoke |= bool(c.test_requests & {'gate-conformance', 'gate-agree'})
            plan.reasons.append(('<resized>', 'the binaries whose pool request changed'))
        if not any(p.startswith(EXAMPLES_INPUTS) for p in self.paths):
            plan.tests = {t for t in plan.tests if not short(t).startswith(EXAMPLES + '-shard-')
                          and short(t) != EXAMPLES}
        plan.add(self.core())
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
            elif token == SMOKE:
                plan.smoke = True
                plan.reasons.append((token, 'chosen'))
            elif token in MAIN_ONLY:
                raise SystemExit(f'gate: --tests {token}: no lane gate runs a whole {token} campaign; main runs it '
                                 f'hourly (tools/main-runs). Choose {SMOKE}, {token}:<path>, or ask for tools/gate --full')
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


def first_failing(count, fails) -> int:
    """The least index below `count` for which `fails` holds, given that it holds
    for `count - 1` and that once it holds it holds for every later index: the
    bisect of tools/land (a batch's prefixes) and tools/main-runs (main's landings)."""
    low, high = -1, count - 1
    while high - low > 1:
        middle = (low + high) // 2
        if fails(middle):
            high = middle
        else:
            low = middle
    return high


def short(target):
    return target.removeprefix('root//:').split(' ')[0]


SHARD = re.compile(r'^(.+)-shard-\d+$')


def failed_cases(details):
    cases = re.findall(r'^test (\S+)(?: - should panic)? \.\.\. FAILED$', details, re.M)
    cases += re.findall(r'^---- (\S+) stdout ----$', details, re.M)
    return list(dict.fromkeys(cases))


# ---------------------------------------------------------------- known failures

# Failures main is known to have, each owned by the lane fixing it: tools/gate
# excludes them and prints every exclusion. A lane that meets a red it did not
# cause, and tools/main-runs for each new red it bisects, push a row straight
# to main; a row expires when its owner lane lands (tools/land removes it).
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


def without_owners(text, owners, listed=None) -> str:
    """The list without the rows these lanes own, of those in `listed` (main's
    list before a landing: a row the landing itself adds stays)."""
    def kept(line):
        fields = line.split('\t')
        return (line.startswith('#') or len(fields) < 3 or fields[2].strip() not in owners
                or (listed is not None and line.rstrip('\n') not in listed))
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


def clippy_report(targets, failed=None) -> tuple[bool, str]:
    """Clippy's diagnostics for the twins the invocation built, adding the
    crates with an error to `failed` when given; fetching the small
    `[clippy.txt]` outputs is a cache hit."""
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
    errors, text = clippy.diagnostics(json.loads(report.stdout or '{}'))
    if failed is not None:
        failed.extend(errors)
    return not errors, text + ('clippy: errors in ' + ', '.join(errors) + '\n' if errors else '')
