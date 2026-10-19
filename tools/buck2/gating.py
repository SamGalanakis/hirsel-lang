"""What tools/gate and tools/land share: a change's paths and identity, the
selection of what it can affect (impact.py), one Buck2 invocation for all of
it, the green record and the failure summary."""

from __future__ import annotations

from concurrent.futures import ThreadPoolExecutor
from dataclasses import dataclass, field
import contextlib
import fcntl
import json
import os
from pathlib import Path
import re
import shlex
import subprocess
import sys
import tempfile
import time
import tomllib

import impact
import event_logs
from sync import (AGREE_LIMIT, AGREE_SHARDS, AGREE_SLICE_PROGRAMS, CONFORMANCE_SHARDS, CONFORMANCE_SLICE_PROGRAMS,
                  SELECTED_SLICES)

ROOT = Path(__file__).resolve().parents[2]
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
    impact.NATIVE_SMOKE: ['root//:gate-native-smoke', 'root//tools/buck2/native-toolchain:proof'],
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


# ---------------------------------------------------------------- gate slots

# `dir:N`: a host's gates run at most N at a time on the pool, each holding a
# lock on one of `dir/slot-0` .. `slot-N-1` while its Buck2 work runs. Unset,
# there is no cap.
GATE_SLOTS = 'HIRSEL_GATE_SLOTS'
SLOT_POLL_SECONDS = 5


def oldest_gate_ticket(queue):
    """Find the earliest live waiter, discarding tickets left by exited gates."""
    live = []
    for ticket in queue.iterdir():
        try:
            stamp, pid = map(int, ticket.name.split('-'))
        except ValueError:
            continue
        if pid <= 0:
            continue
        try:
            os.kill(pid, 0)
        except ProcessLookupError:
            ticket.unlink(missing_ok=True)
            continue
        except PermissionError:
            pass  # A live process owned by another user.
        live.append((stamp, pid, ticket))
    return min(live)[2] if live else None


@contextlib.contextmanager
def gate_slot(environ=os.environ):
    """Hold one of the host's gate slots, in FIFO waiter order."""
    spec = environ.get(GATE_SLOTS, '')
    if not spec:
        yield
        return
    directory, _, count = spec.rpartition(':')
    if not directory or not count.isdigit() or int(count) < 1:
        raise SystemExit(f'gate: {GATE_SLOTS}={spec!r} is not dir:N')
    directory = Path(directory)
    directory.mkdir(parents=True, exist_ok=True)
    queue = directory / 'queue'
    queue.mkdir(exist_ok=True)
    while True:
        ticket = queue / f'{time.time_ns()}-{os.getpid()}'
        try:
            ticket.touch(exist_ok=False)
            break
        except FileExistsError:
            continue
    waiting = None
    try:
        while True:
            if oldest_gate_ticket(queue) == ticket:
                for slot in range(int(count)):
                    # Python opens files non-inheritable, so Buck2 never holds the lock.
                    handle = open(directory / f'slot-{slot}', 'a+')
                    try:
                        fcntl.flock(handle, fcntl.LOCK_EX | fcntl.LOCK_NB)
                    except BlockingIOError:
                        handle.close()
                        continue
                    with handle:
                        handle.truncate(0)
                        handle.write(f'{os.getpid()} {ROOT}\n')
                        handle.flush()
                        ticket.unlink(missing_ok=True)
                        if waiting is not None:
                            say(f'gate: took gate slot {slot} after waiting {time.monotonic() - waiting:.0f} s')
                        yield
                    return
            if waiting is None:
                holders = [path.read_text().strip() if path.exists() else ''
                           for slot in range(int(count))
                           for path in [directory / f'slot-{slot}']]
                say(f'gate: waiting for one of {count} gate slots in {directory}; held by: ' + '; '.join(holders))
                waiting = time.monotonic()
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


def record_green(identity, crates, root=ROOT, **record):
    """Remember a passed change: its crates, when, and what the gate ran
    (`selection`, `why`, `floor`) with the per-path digests `--extend` compares."""
    records = green_records(root)
    records[identity] = dict(record, crates=sorted(crates), at=time.strftime('%Y-%m-%dT%H:%M:%SZ', time.gmtime()))
    path = green_path(root)
    temporary = path.with_name(path.name + f'.{os.getpid()}')
    temporary.write_text(json.dumps(records, indent=1, sort_keys=True) + '\n')
    temporary.replace(path)


def latest_green(root=ROOT):
    """The most recently recorded passed change of this worktree: (identity, record)."""
    records = [(record.get('at', ''), identity, record) for identity, record in green_records(root).items()]
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
    return bool(identity) and record is not None and record['crates'] == sorted(crates)


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
        return event_logs.run([command, *CONFIG, *arguments], buck2=BUCK2, cwd=ROOT, **kwargs)
    with tempfile.NamedTemporaryFile('w', prefix='hirsel-config-', suffix='.args') as argfile:
        argfile.write('\n'.join(SESSION_CONFIG) + '\n')
        argfile.flush()
        return event_logs.run([command, *CONFIG, f'@{argfile.name}', *arguments], buck2=BUCK2, cwd=ROOT, **kwargs)


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


def macro_targets(names) -> str:
    """The targets each generated rule call declares: its name and the harness,
    host and shard targets a hirsel_test adds."""
    pattern = '|'.join(re.escape(name) for name in sorted(names))
    return f'attrregexfilter(name, "^({pattern})(-harness|-host|-shard-[0-9]+)?$", root//:)'


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


def crate_tests(crates) -> str:
    """The query for every test target of these crates: unit, integration (all
    shards), binary unit tests and doctests."""
    names = '|'.join(re.escape(crate) for crate in sorted(crates))
    return (f'(kind("_rust_test", attrregexfilter(name, "^({names})-(unittest|test-.+|bin-.+-unittest)$", root//:))'
            f' + attrfilter(labels, doctest, attrregexfilter(name, "^({names})$", root//:)))'
            ' - attrfilter(labels, gate, //...)')


def corpus_cases(prefix, root=ROOT) -> list[str]:
    """The corpus cases under a repository path prefix (a case names itself)."""
    cases = sorted(p.relative_to(root).as_posix() for p in (root / 'conformance').rglob('*.hir'))
    return [case for case in cases if case.startswith(prefix)]


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
            plan.tests |= set(uquery(crate_tests(c.crates)))
            plan.reasons.append((', '.join(sorted(c.crates)), 'the crates whose source changed: all their tests'))
        tests = [p for p in c.owned if p.startswith('crates/') and p.split('/')[1] not in c.crates]
        if tests:
            files = owned_paths(tests, self.root)
            if files:
                owners = ' + '.join(f'owner("{path}")' for path in files)
                plan.tests |= set(uquery(f'kind("_rust_test", {owners}) - attrfilter(labels, gate, //...)'))
                plan.reasons.append(('test code', 'the test targets of the changed test files'))
        cases = impact.changed_programs(self.paths, self.root, self.ratchet())
        if cases:
            plan.conformance |= set(cases)
            plan.reasons.append((CONFORMANCE_PREFIX + '<changed>', f'{len(cases)} changed or added corpus cases and their twins'))
        if dependents := self.std_dependents():
            # std is source too: its tests are the cases that import the changed modules.
            plan.conformance |= set(dependents)
            plan.reasons.append((CONFORMANCE_PREFIX + '<std>', self.std_why))
        return plan

    def std_examples(self) -> list[str]:
        """The examples whose import closure includes a changed std module."""
        if not self.std_dependents():
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
            plan.tests |= set(uquery(ALL_TESTS))
            plan.campaigns |= set(impact.CAMPAIGNS)
            plan.reasons.append((f'{ALL} conformance agreement native-smoke',
                                 'a build-wide input, an unknown crate or the BUCK header changed'))
            return plan
        scopes = []
        data = [p for p in c.owned if not p.startswith('crates/')]
        if c.owned:
            files = owned_paths(c.owned, root)
            if files:
                scopes.append(' + '.join(f'owner("{path}")' for path in files))
        if isinstance(c.buck, set) and c.buck:
            scopes.append(macro_targets(c.buck))
        if scopes:
            scope = f'rdeps(//..., {" + ".join(scopes)})'
            plan.tests |= set(uquery(f'(kind("_rust_test", {scope}) + attrfilter(labels, doctest, {scope}))'
                                     ' - attrfilter(labels, gate, //...)'))
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
            resized = uquery(f'kind("_rust_test", {macro_targets(c.test_requests)})'
                             ' - attrfilter(labels, gate, //...)')
            plan.tests |= set(resized)
            for campaign, targets in CAMPAIGN_TARGETS.items():
                if any(re.sub(r'-shard-\d+$', '', short(t)) in c.test_requests for t in targets):
                    plan.campaigns.add(campaign)
            plan.reasons.append(('<resized>', 'the binaries whose pool request changed'))
        if not any(p.startswith(EXAMPLES_INPUTS) for p in self.paths):
            plan.tests = {t for t in plan.tests if not short(t).startswith(EXAMPLES + '-shard-')
                          and short(t) != EXAMPLES}
            if users := self.std_examples():
                plan.reasons.append(('(not suggested)', f'{EXAMPLES} runs the examples that reach the changed std '
                                     f'modules ({", ".join(users)}); add it when the change alters what they print'))
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
                plan.tests |= set(uquery(ALL_TESTS))
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
                if not (self.root / program).is_file():
                    raise SystemExit(f'gate: --tests {token}: {program} is no file')
                plan.native.add(program)
                plan.reasons.append((token, 'chosen'))
            else:
                if names is None:
                    names = {short(t) for t in uquery(ALL_TESTS)}
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


def test_results(event_log) -> dict[str, dict]:
    """Each test's TestResult event from a `buck2 test` event log, by short name."""
    events = subprocess.run([str(BUCK2), 'log', 'show', str(event_log)], cwd=ROOT, check=True,
                            stdout=subprocess.PIPE, text=True, errors='replace').stdout
    results = {}
    for line in events.splitlines():
        if '"TestResult"' not in line:
            continue
        data = json.loads(line).get('Event', {}).get('data', {}).get('Instant', {}).get('data', {})
        if result := data.get('TestResult'):
            results[short(result['name'])] = dict(status=result.get('status'), details=result.get('details') or '',
                                                  seconds=(result.get('duration_us') or 0) / 1e6)
    return results


def short(target):
    return target.removeprefix('root//:').split(' ')[0]


SHARD = re.compile(r'^(.+)-shard-\d+$')


def failed_cases(details):
    cases = re.findall(r'^test (\S+)(?: - should panic)? \.\.\. FAILED$', details, re.M)
    cases += re.findall(r'^---- (\S+) stdout ----$', details, re.M)
    return list(dict.fromkeys(cases))


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

        def run():
            result = subprocess.run(command, cwd=cwd, capture_output=True, text=True)
            return result.returncode, result.stdout + result.stderr, time.monotonic() - started
        self.jobs.append((name, command, self.pool.submit(run)))

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
