#!/usr/bin/env python3
"""tools/land against a scratch origin with fake Buck2 checks: a push rejected
because main moved goes back to the rebase, and a red check refuses the push."""
import json
import os
import runpy
from types import SimpleNamespace
from unittest.mock import patch
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
# The harness removes the randomized delay only; production always uses jitter.
DRIVER = '''import runpy, sys
from unittest.mock import patch
module = runpy.run_path('tools/land', run_name='land_test')
with patch('time.sleep', lambda seconds: print('test: jitter', seconds, flush=True)):
    sys.exit(module['main']())
'''
RECORD = '''import os, pathlib, subprocess, sys
root = pathlib.Path(os.environ['FAKE_CHECKS'])
head = subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip()
base = subprocess.check_output(['git', 'rev-parse', 'origin/main'], text=True).strip()
folder = root / head
folder.mkdir(parents=True, exist_ok=True)
(folder / STAGE).write_text(base)
if os.environ.get('FAKE_RED_AFTER_RACE') and STAGE == os.environ['FAKE_RED_AFTER_RACE'] and (root / 'race-0').exists():
    sys.exit(1)
'''

FAKES = {
    '.buck2/bin/buck2': '#!/bin/sh\necho root//:a\n',
    'tools/buck2/sync.py': '',
    'tools/check-hot-files': '',
    'tools/buck2/event_logs.py': '',
    # The first Clippy run lands another lane's commit on origin, as a lane landing meanwhile would.
    'tools/buck2/clippy.py': '''import os, subprocess
other = os.environ.get('FAKE_OTHER_LANE')
if other and not os.path.exists(other):
    subprocess.run(['git', 'clone', '-q', os.environ['FAKE_ORIGIN'], other], check=True)
    open(os.path.join(other, 'other.txt'), 'w').write('other\\n')
    subprocess.run(['git', 'add', 'other.txt'], cwd=other, check=True)
    subprocess.run(['git', 'commit', '-qm', 'other'], cwd=other, check=True)
    subprocess.run(['git', 'push', '-q', 'origin', 'HEAD:main'], cwd=other, check=True)
''',
    'tools/buck2/fmt.py': '''import os, sys
if os.environ.get('FAKE_FMT_RED'):
    print('checking 3 files')
    print('Diff in crates/a/src/lib.rs at line 1:')
    sys.exit(1)
''',
}


class Land(unittest.TestCase):
    def setUp(self):
        self.top = Path(tempfile.mkdtemp(prefix='land-test-'))
        self.addCleanup(shutil.rmtree, self.top)
        self.origin, self.lane = self.top / 'origin.git', self.top / 'lane'
        self.env = {k: v for k, v in os.environ.items() if not k.startswith(('GIT_', 'HIRSEL_'))} | dict(
            GIT_AUTHOR_NAME='t', GIT_AUTHOR_EMAIL='t@t', GIT_COMMITTER_NAME='t', GIT_COMMITTER_EMAIL='t@t',
            FAKE_ORIGIN=str(self.origin))
        self.git(self.top, 'init', '-q', '--bare', '-b', 'main', str(self.origin))
        self.git(self.top, 'clone', '-q', str(self.origin), str(self.lane))
        files = dict(FAKES, **{'tools/land': (ROOT / 'tools/land').read_text(),
                               'tools/buck2/generated.py': (ROOT / 'tools/buck2/generated.py').read_text(),
                               'tools/buck2/merging.py': (ROOT / 'tools/buck2/merging.py').read_text(),
                               '.gitignore': '.buck2/\n'})
        for name, text in files.items():
            (self.lane / name).parent.mkdir(parents=True, exist_ok=True)
            (self.lane / name).write_text(text)
        (self.lane / '.buck2/bin/buck2').chmod(0o755)
        self.git(self.lane, 'checkout', '-q', '-b', 'main')
        self.git(self.lane, 'add', '-A')
        self.git(self.lane, 'commit', '-qm', 'base')
        self.git(self.lane, 'push', '-q', 'origin', 'main')
        self.git(self.lane, 'checkout', '-q', '-b', 'my-lane')
        (self.lane / 'change.txt').write_text('change\n')
        self.git(self.lane, 'add', 'change.txt')
        self.git(self.lane, 'commit', '-qm', 'change')

    def git(self, cwd, *arguments):
        return subprocess.run(['git', *arguments], cwd=cwd, env=self.env, check=True, capture_output=True,
                              text=True).stdout

    def land(self, **env):
        return subprocess.run([sys.executable, '-c', DRIVER], cwd=self.lane, env=self.env | env,
                              capture_output=True, text=True)

    def main_log(self):
        return self.git(self.origin, 'log', '--format=%s', 'main').split('\n')[:-1]

    def test_a_push_rejected_because_main_moved_rebases_rebuilds_and_lands(self):
        landed = self.land(FAKE_OTHER_LANE=str(self.top / 'other'))
        self.assertEqual(landed.returncode, 0, landed.stdout + landed.stderr)
        self.assertIn('main moved during the checks; rebasing again', landed.stdout)
        # Every attempted tree passes all three checks, including a retry.
        self.assertEqual(landed.stdout.count('pass  workspace build'), 2)
        self.assertEqual(landed.stdout.count('pass  Clippy'), 2)
        self.assertEqual(self.main_log(), ['change', 'other', 'base'])
        [row] = [json.loads(line) for line in (self.lane / '.git/hirsel-landings.jsonl').read_text().splitlines()]
        self.assertEqual((row['branch'], row['landed']), ('my-lane', self.git(self.origin, 'rev-parse', 'main').strip()))

    def recording_remote(self):
        checks = self.top / 'checks'
        checks.mkdir()
        self.env['FAKE_CHECKS'] = str(checks)
        # A real receive hook verifies every proposed tree has all checks,
        # and every accepted tree was checked on the remote's exact old tip.
        hook = self.origin / 'hooks/pre-receive'
        hook.write_text("""#!/usr/bin/env python3
import os, pathlib, subprocess, sys
root = pathlib.Path(os.environ['FAKE_CHECKS'])
for line in sys.stdin:
    old, new, ref = line.split()
    stages = [root / new / stage for stage in ('build', 'clippy', 'fmt')]
    if not all(path.is_file() for path in stages):
        print('UNCHECKED TREE', new, file=sys.stderr)
        sys.exit(1)
    if any(path.read_text() != old for path in stages):
        print('WRONG CHECKED BASE', new, file=sys.stderr)
        sys.exit(1)
    with (root / 'received').open('a') as out:
        out.write(new + '\\n')
""")
        hook.chmod(0o755)
        for name, stage in [('event_logs', 'build'), ('clippy', 'clippy'), ('fmt', 'fmt')]:
            (self.lane / f'tools/buck2/{name}.py').write_text('STAGE = ' + repr(stage) + '\n' + RECORD)
        self.git(self.lane, 'add', 'tools/buck2')
        self.git(self.lane, 'commit', '-qm', 'record checks')
        return checks

    def test_two_concurrent_lands_check_every_tree_and_eventually_land(self):
        checks = self.recording_remote()
        # Both clones inherit identical tools, then add independent lane changes.
        other = self.top / 'other'
        self.git(self.top, 'clone', '-q', str(self.lane), str(other))
        self.git(other, 'remote', 'set-url', 'origin', str(self.origin))
        self.git(other, 'fetch', '-q', 'origin')
        (other / '.buck2/bin').mkdir(parents=True)
        shutil.copy(self.lane / '.buck2/bin/buck2', other / '.buck2/bin/buck2')
        (other / 'other.txt').write_text('other\n')
        self.git(other, 'add', 'other.txt')
        self.git(other, 'commit', '-qm', 'other')
        # The two proposals contain independent changes, so either winner
        # leaves the loser real work to rebase, check again and push.
        (self.lane / 'first.txt').write_text('first\n')
        self.git(self.lane, 'add', 'first.txt')
        self.git(self.lane, 'commit', '-qm', 'first lane')
        # A barrier ensures both initial trees pass checks before either pushes.
        barrier = """
if STAGE == 'fmt':
    import time
    marker = root / ('ready-' + pathlib.Path.cwd().name)
    marker.touch()
    deadline = time.monotonic() + 15
    while len(list(root.glob('ready-*'))) < 2:
        if time.monotonic() > deadline:
            raise SystemExit('barrier timed out')
        time.sleep(.01)
"""
        # Patch only the harness's stage scripts, and commit the same barrier in both.
        for lane in (self.lane, other):
            with (lane / 'tools/buck2/fmt.py').open('a') as out:
                out.write(barrier)
            self.git(lane, 'add', 'tools/buck2/fmt.py')
            self.git(lane, 'commit', '-qm', 'check barrier')
        processes = [subprocess.Popen([sys.executable, '-c', DRIVER], cwd=lane,
                                      env=self.env, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
                     for lane in (self.lane, other)]
        outputs = []
        try:
            for proc in processes:
                out, err = proc.communicate(timeout=30)
                outputs.append(out)
                self.assertEqual(proc.returncode, 0, out + err)
        finally:
            for proc in processes:
                if proc.poll() is None:
                    proc.kill()
                    proc.communicate()
        self.assertEqual(sum(out.count('main moved during the checks') for out in outputs), 1)
        self.assertEqual(sum(out.count('pass  Clippy') for out in outputs), 3)
        self.assertEqual(sum(out.count('pass  formatting') for out in outputs), 3)
        self.assertIn('other', self.main_log())
        self.assertIn('first lane', self.main_log())
        self.assertEqual(self.git(self.origin, 'show', 'main:first.txt'), 'first\n')
        self.assertEqual(self.git(self.origin, 'show', 'main:other.txt'), 'other\n')
        # The loser may reach pre-receive before losing the ref lock; all
        # three checked proposals are valid, and both commits reach main.
        self.assertGreaterEqual(len((checks / 'received').read_text().splitlines()), 2)

    def test_more_than_twenty_lost_races_recheck_all_stages_and_land(self):
        checks = self.recording_remote()
        # The fake remote declines twenty-two fully checked pushes as races.
        hook = self.origin / 'hooks/pre-receive'
        with hook.open('a') as out:
            out.write("""
for i in range(22):
    marker = root / ('race-' + str(i))
    if not marker.exists():
        marker.touch()
        print('[rejected] simulated sibling won', file=sys.stderr)
        sys.exit(1)
""")
        landed = self.land()
        self.assertEqual(landed.returncode, 0, landed.stdout + landed.stderr)
        for stage in ('workspace build', 'Clippy', 'formatting'):
            self.assertEqual(landed.stdout.count('pass  ' + stage), 23)
        self.assertEqual(landed.stdout.count('test: jitter'), 22)
        self.assertNotIn('unchecked', landed.stdout)

    def test_a_red_retry_never_pushes_for_each_aggregate(self):
        for stage in ('build', 'clippy', 'fmt'):
            with self.subTest(stage=stage):
                checks = self.recording_remote() if stage == 'build' else self.top / 'checks'
                hook = self.origin / 'hooks/pre-receive'
                with hook.open('a') as out:
                    out.write("""
marker = root / 'race-0'
if not marker.exists():
    marker.touch()
    print('[rejected] simulated sibling won', file=sys.stderr)
    sys.exit(1)
""")
                (checks / 'race-0').unlink(missing_ok=True)
                before = self.git(self.origin, 'rev-parse', 'main')
                refused = self.land(FAKE_RED_AFTER_RACE=stage)
                self.assertEqual(refused.returncode, 1, refused.stdout + refused.stderr)
                self.assertIn('refusing to push a red tree', refused.stdout)
                self.assertEqual(self.git(self.origin, 'rev-parse', 'main'), before)

    def test_head_or_source_change_during_checks_refuses_the_push(self):
        for command in ("open('change.txt', 'a').write('edit')",
                        "subprocess.run(['git', 'commit', '--allow-empty', '-qm', 'moved'], check=True)"):
            with self.subTest(command=command):
                (self.lane / 'tools/buck2/fmt.py').write_text('import subprocess\n' + command + '\n')
                self.git(self.lane, 'add', '-A')
                self.git(self.lane, 'commit', '-qm', 'mutate during check')
                refused = self.land()
                self.assertEqual(refused.returncode, 1, refused.stdout + refused.stderr)
                self.assertIn('worktree changed during checks', refused.stdout)
                self.assertEqual(self.main_log(), ['base'])

    def test_retry_scope_requires_exact_disjoint_existing_rust_changes(self):
        files = {'crates/a/Cargo.toml': '[package]\nname = "a"\n',
                 'crates/a/src/lib.rs': 'pub fn a() {}\n',
                 'crates/b/Cargo.toml': '[package]\nname = "b"\n',
                 'crates/b/src/lib.rs': 'pub fn b() {}\n'}
        for name, text in files.items():
            path = self.lane / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(text)
        self.git(self.lane, 'add', '-A')
        self.git(self.lane, 'commit', '-qm', 'proof base')
        old_base = self.git(self.lane, 'rev-parse', 'HEAD').strip()
        (self.lane / 'crates/b/src/lib.rs').write_text('pub fn b() { /* lane */ }\n')
        self.git(self.lane, 'commit', '-qam', 'own Rust change')
        old_head = self.git(self.lane, 'rev-parse', 'HEAD').strip()
        cases = [
            ('crates/a/src/lib.rs', 'pub fn a() { /* incoming */ }\n', True),
            ('crates/a/Cargo.toml', '[package]\nname = "a"\nversion = "1"\n', False),
            ('crates/a/src/new.rs', 'pub fn new() {}\n', False),
            ('crates/a/src/lib.rs', None, False),
            ('crates/a/build.rs', 'fn main() {}\n', False),
            ('docs/notes.md', 'docs\n', False),
            ('crates/b/src/lib.rs', 'pub fn b() { /* lane */ }\n', False),
        ]
        for name, text, narrow in cases:
            with self.subTest(path=name, text=text):
                self.git(self.lane, 'checkout', '-q', '--detach', old_base)
                path = self.lane / name
                if text is None:
                    path.unlink()
                else:
                    path.parent.mkdir(parents=True, exist_ok=True)
                    path.write_text(text)
                self.git(self.lane, 'add', '-A')
                self.git(self.lane, 'commit', '-qm', 'incoming')
                base = self.git(self.lane, 'rev-parse', 'HEAD').strip()
                self.git(self.lane, 'checkout', '-q', '--detach', old_head)
                self.git(self.lane, 'rebase', base)
                head = self.git(self.lane, 'rev-parse', 'HEAD').strip()
                code = ("import runpy, json; m=runpy.run_path('tools/land'); "
                        f"print(json.dumps(m['retry_scope'](({old_base!r}, {old_head!r}), {base!r}, {head!r})))")
                result = subprocess.run([sys.executable, '-c', code], cwd=self.lane,
                                        env=self.env, check=True, capture_output=True, text=True)
                scope = json.loads(result.stdout)
                self.assertEqual(scope is not None, narrow, scope)
                if narrow:
                    self.assertIn('rdeps(', scope)
                    self.assertIn('owner("crates/a/src/lib.rs")', scope)

    def test_legacy_linted_environment_cannot_bypass_checks(self):
        refused = self.land(HIRSEL_LAND_LINTED='1', HIRSEL_LAND_ATTEMPT='30', FAKE_FMT_RED='1')
        self.assertEqual(refused.returncode, 1, refused.stdout + refused.stderr)
        self.assertIn('pass  workspace build', refused.stdout)
        self.assertIn('pass  Clippy', refused.stdout)
        self.assertIn('FAIL  formatting', refused.stdout)
        self.assertEqual(self.main_log(), ['base'])

    def conflicting_lanes(self, path):
        # Land has two source commits to replay, both carry a generated edit.
        self.git(self.lane, 'checkout', '-q', 'main')
        target = self.lane / path
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text('base\n')
        self.git(self.lane, 'add', path)
        self.git(self.lane, 'commit', '-qm', 'common generated base')
        self.git(self.lane, 'checkout', '-q', 'my-lane')
        self.git(self.lane, 'rebase', 'main')
        for i in range(2):
            target.write_text(f'lane {i}\n')
            (self.lane / f'source-{i}').write_text('source change\n')
            self.git(self.lane, 'add', '-A')
            self.git(self.lane, 'commit', '-qm', f'change {i}')
        self.git(self.lane, 'checkout', '-q', 'main')
        target.write_text('main generator output\n')
        self.git(self.lane, 'add', path)
        self.git(self.lane, 'commit', '-qm', 'main generator')
        self.git(self.lane, 'push', '-q', 'origin', 'main')
        self.git(self.lane, 'checkout', '-q', 'my-lane')

    def test_generated_pages_keep_mains_writer_and_preserve_source_commits(self):
        self.conflicting_lanes('docs/api.md')
        landed = self.land()
        self.assertEqual(landed.returncode, 0, landed.stdout + landed.stderr)
        self.assertEqual(landed.stdout.count('resolved generated conflict in docs/api.md'), 2)
        self.assertEqual((self.lane / 'docs/api.md').read_text(), 'main generator output\n')
        self.assertEqual(self.main_log()[:2], ['change 1', 'change 0'])
        for i in range(2):
            self.assertEqual((self.lane / f'source-{i}').read_text(), 'source change\n')

    def test_a_generated_only_conflict_drops_the_obsolete_generated_commit(self):
        self.conflicting_lanes('docs/api.md')
        (self.lane / 'docs/api.md').write_text('obsolete generated-only page\n')
        self.git(self.lane, 'add', 'docs/api.md')
        self.git(self.lane, 'commit', '-qm', 'obsolete generated page')
        landed = self.land()
        self.assertEqual(landed.returncode, 0, landed.stdout + landed.stderr)
        self.assertNotIn('obsolete generated page', self.main_log())
        self.assertEqual((self.lane / 'docs/api.md').read_text(), 'main generator output\n')

    def test_generated_build_inventory_is_untracked_and_regenerated(self):
        self.conflicting_lanes('BUCK')
        (self.lane / 'tools/buck2/sync.py').write_text(
            "from pathlib import Path\nPath('BUCK').write_text('fresh inventory\\n')\n")
        with open(self.lane / '.gitignore', 'a') as ignore:
            ignore.write('BUCK\n')
        self.git(self.lane, 'add', 'tools/buck2/sync.py', '.gitignore')
        self.git(self.lane, 'commit', '-qm', 'generator')
        landed = self.land()
        self.assertEqual(landed.returncode, 0, landed.stdout + landed.stderr)
        self.assertEqual((self.lane / 'BUCK').read_text(), 'fresh inventory\n')
        self.assertEqual(self.git(self.lane, 'ls-files', 'BUCK'), '')

    def test_source_conflicts_still_refuse_the_push_and_abort_the_rebase(self):
        self.conflicting_lanes('source.rs')
        refused = self.land()
        self.assertEqual(refused.returncode, 1, refused.stdout + refused.stderr)
        self.assertIn('conflicts in: source.rs', refused.stdout)
        self.assertEqual(self.git(self.lane, 'branch', '--show-current').strip(), 'my-lane')

    def test_a_red_check_refuses_the_push_and_prints_its_first_error(self):
        refused = self.land(FAKE_FMT_RED='1')
        self.assertEqual(refused.returncode, 1, refused.stdout + refused.stderr)
        self.assertIn('FAIL  formatting', refused.stdout)
        self.assertIn('Diff in crates/a/src/lib.rs', refused.stdout)
        self.assertNotIn('checking 3 files', refused.stdout)
        self.assertIn('refusing to push a red tree', refused.stdout)
        self.assertEqual(self.main_log(), ['base'])


class RetryChecks(unittest.TestCase):
    def test_uncertain_graph_or_inventory_runs_full_aggregates(self):
        module = runpy.run_path(str(ROOT / 'tools/land'))
        checks = module['checks']
        for before, after, graph in ((None, ('a',), 'root//:a'),
                                     (('a',), ('b',), 'root//:a'),
                                     (('a',), ('a',), ''),
                                     (('a',), ('a',), 'root//:a')):
            with self.subTest(before=before, after=after, graph=graph):
                commands = []
                def check(name, command):
                    commands.append(command)
                    return True
                def query(command, **kwargs):
                    return SimpleNamespace(returncode=0, stdout=graph if command[-1] == 'scope' else 'root//:a', stderr='')
                with patch.dict(checks.__globals__, inventory=lambda: after, check=check), patch('subprocess.run', query):
                    self.assertTrue(checks('scope', before))
                scoped = [command for command in commands if '--scope' in command]
                self.assertEqual(len(scoped), 2 if before == after and graph else 0)
                self.assertEqual(len(commands), 4)  # sync, build, Clippy, fmt

    def test_missing_or_malformed_commit_cannot_become_a_push_refspec(self):
        module = runpy.run_path(str(ROOT / 'tools/land'))
        resolve = module['revision']
        for status, value in ((1, ''), (0, ''), (0, 'HEAD')):
            with self.subTest(status=status, value=value):
                with patch.dict(resolve.__globals__, git=lambda *args: SimpleNamespace(
                        returncode=status, stdout=value, stderr='missing')):
                    with self.assertRaises(ValueError):
                        resolve('HEAD')

    def test_empty_build_query_refuses_the_aggregate(self):
        module = runpy.run_path(str(ROOT / 'tools/land'))
        checks = module['checks']
        with patch.dict(checks.__globals__, check=lambda *args: True), patch('subprocess.run') as query:
            query.return_value = SimpleNamespace(returncode=0, stdout='', stderr='')
            self.assertFalse(checks())

    def test_scoped_clippy_still_rejects_error_diagnostics(self):
        module = runpy.run_path(str(ROOT / 'tools/buck2/clippy.py'))
        main = module['main']
        with tempfile.TemporaryDirectory() as directory:
            report = Path(directory) / 'clippy.txt'
            report.write_text('error[E0063]: missing field\n')
            built = SimpleNamespace(returncode=0, stdout=json.dumps({'root//:a-clippy[clippy.txt]': str(report)}), stderr='')
            with patch.object(sys, 'argv', ['clippy.py', '--scope', 'scope']), patch('subprocess.run') as query, \
                    patch.object(main.__globals__['event_logs'], 'run', return_value=built):
                query.return_value = SimpleNamespace(returncode=0, stdout='root//:a-clippy')
                self.assertEqual(main(), 1)
                self.assertIn('intersect (scope)', query.call_args.args[0][-1])

    def test_empty_clippy_or_formatting_scope_is_never_green(self):
        for helper, output in (('clippy', ''), ('fmt', '{}')):
            with self.subTest(helper=helper):
                module = runpy.run_path(str(ROOT / f'tools/buck2/{helper}.py'))
                with patch.object(sys, 'argv', [helper, '--scope', 'scope']), patch('subprocess.run') as query:
                    query.return_value = SimpleNamespace(returncode=0, stdout=output)
                    with self.assertRaises(SystemExit):
                        module['main']()


if __name__ == '__main__':
    unittest.main()
