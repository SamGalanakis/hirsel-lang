"""What tools/gate and tools/land share: a change's paths and identity, the
selection of what it can affect (impact.py), one Buck2 invocation for all of
it, the green record and the failure summary."""

from __future__ import annotations

from concurrent.futures import ThreadPoolExecutor
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
from sync import AGREE_SHARDS, CONFORMANCE_SHARDS

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
    impact.NATIVE_SMOKE: ['root//:gate-native-smoke'],
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
    # These unit-test harnesses read the §22
    # signatures from BuiltinRegistry, which is built with check and modes.
    # hirsel-signature and hirsel-lower have no registry-backed harnesses.
    'hirsel-check': 'body-check harnesses read BuiltinRegistry signatures',
    'hirsel-modes': 'authority and ownership harnesses read BuiltinRegistry signatures',
}


def say(message):
    print(message, flush=True)


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


def change_identity(base, root=ROOT) -> str:
    """The patch-id of the change from `base` to the worktree, from a diff without
    context lines: a clean rebase onto a newer main keeps it, and any edit of the
    change, including a hand-resolved conflict, gives a new one. Empty when the
    worktree does not differ from `base`."""
    diff = subprocess.run(['git', 'diff', '-U0', '--binary', '--no-renames', base, worktree_tree(root)],
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


def record_green(identity, crates, root=ROOT):
    records = green_records(root)
    records[identity] = dict(crates=sorted(crates), at=time.strftime('%Y-%m-%dT%H:%M:%SZ', time.gmtime()))
    path = green_path(root)
    temporary = path.with_name(path.name + f'.{os.getpid()}')
    temporary.write_text(json.dumps(records, indent=1, sort_keys=True) + '\n')
    temporary.replace(path)


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
    if missing := sorted(crates - impact.CRATE_CAMPAIGNS.keys()):
        errors.append('tools/buck2/impact.py CRATE_CAMPAIGNS has no row for: ' + ', '.join(missing))
    if stale := sorted(impact.CRATE_CAMPAIGNS.keys() - crates):
        errors.append('tools/buck2/impact.py CRATE_CAMPAIGNS names crates that are gone: ' + ', '.join(stale))
    return errors


def owned_paths(paths, root=ROOT):
    """The files to ask Buck2 about: a deleted file stands for its directory's files."""
    files = set()
    for path in paths:
        if (root / path).is_file():
            files.add(path)
            continue
        directory = (root / path).parent
        while not directory.is_dir():
            directory = directory.parent
        files.update(p.relative_to(root).as_posix() for p in directory.iterdir() if p.is_file())
    return sorted(files)


# ---------------------------------------------------------------- Buck2

def buck2(command, *arguments, **kwargs):
    return subprocess.run([str(BUCK2), command, *CONFIG, *arguments], cwd=ROOT, **kwargs)


def uquery(expression) -> list[str]:
    result = buck2('uquery', expression, capture_output=True, text=True)
    if result.returncode:
        raise SystemExit(f'buck2 uquery failed: {expression}\n{result.stderr}')
    return result.stdout.split()


def macro_targets(names) -> str:
    """The targets each generated rule call declares: its name and the harness,
    host and shard targets a hirsel_test adds."""
    pattern = '|'.join(re.escape(name) for name in sorted(names))
    return f'attrregexfilter(name, "^({pattern})(-harness|-host|-shard-[0-9]+)?$", root//:)'


class Selection:
    """What a change runs."""

    def __init__(self, base, root=ROOT):
        self.base = base
        self.paths = changed_paths(base, root)
        buck_text = (root / 'BUCK').read_text()
        self.classified = impact.classify(self.paths, members(root), buck_text)
        if 'BUCK' in self.paths:
            old = subprocess.run(['git', 'show', f'{base}:BUCK'], cwd=root, capture_output=True, text=True).stdout
            self.classified.buck = impact.buck_targets(old, buck_text)
        self.tests: list[str] = []
        self.campaigns: set[str] = set()

    @property
    def crates(self):
        return self.classified.crates

    def resolve(self):
        """Ask Buck2 which tests and campaigns the change reaches."""
        c = self.classified
        if c.everything:
            self.tests = uquery(ALL_TESTS)
            self.campaigns = set(impact.CAMPAIGNS)
            return
        scopes = []
        if c.owned:
            files = owned_paths(c.owned)
            owners = ' + '.join(f'owner("{path}")' for path in files)
            if owners:
                scopes.append(owners)
                # A campaign reads its data files directly (the corpus, the stdlib, native fixtures).
                direct = set(uquery(f'attrfilter(labels, gate, {owners})'))
                for campaign, targets in CAMPAIGN_TARGETS.items():
                    if direct & set(targets):
                        self.campaigns.add(campaign)
        if isinstance(c.buck, set) and c.buck:
            scopes.append(macro_targets(c.buck))
        self.campaigns |= c.campaigns
        if scopes:
            scope = f'rdeps(//..., {" + ".join(scopes)})'
            self.tests = uquery(f'(kind("_rust_test", {scope}) + attrfilter(labels, doctest, {scope}))'
                                ' - attrfilter(labels, gate, //...)')

    def campaign_targets(self):
        return [t for campaign in impact.CAMPAIGNS if campaign in self.campaigns
                for t in CAMPAIGN_TARGETS[campaign]]


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


def rerun(target, details) -> list[str]:
    """What failed in one test target, and the commands that rerun it alone."""
    buck = '.buck2/bin/buck2 test'
    if target.startswith('gate-conformance-shard-'):
        lines = re.findall(r'^(conformance/\S+\.hir: .*)$', details, re.M)
        return lines + [f'{buck} //:{target}']
    if target.startswith('gate-agree-shard-'):
        lines = []
        for match in re.finditer(r'(?m)^\{', details):
            try:
                report, _ = json.JSONDecoder().raw_decode(details[match.start():])
            except ValueError:
                continue
            for cluster in report.get('clusters', []) if isinstance(report, dict) else []:
                path = cluster.get('path', '?')
                lines.append(f'{cluster.get("kind", "discrepancy")}: {path}')
                lines.append('.buck2/bin/buck2 run --target-platforms //tools/buck2:release '
                             f'//:hirsel-conformance-bin-agree -- --root "$PWD" --seed 1 --program {path}')
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
    """The gate tooling's own tests, each its own command."""
    tests = [(f'tools/tests/{p.name}', [sys.executable, str(p)]) for p in sorted((ROOT / 'tools/tests').glob('*.py'))]
    return tests + [('conformance.py --self-test', [sys.executable, str(ROOT / 'tools/buck2/conformance.py'), '--self-test'])]


def clippy_report(targets) -> tuple[bool, str]:
    """Clippy's diagnostics for the twins the invocation built; fetching the
    small `[clippy.txt]` outputs is a cache hit."""
    import clippy
    report = buck2('build', '--show-full-json-output', *[t + '[clippy.txt]' for t in targets],
                   capture_output=True, text=True)
    if report.returncode:
        return False, report.stderr
    failed, text = clippy.diagnostics(json.loads(report.stdout or '{}'))
    return not failed, text + ('clippy: errors in ' + ', '.join(failed) + '\n' if failed else '')
