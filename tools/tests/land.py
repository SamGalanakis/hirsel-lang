#!/usr/bin/env python3
"""tools/land against a scratch origin with fake Buck2 checks: a push rejected
because main moved goes back to the rebase, and a red check refuses the push."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
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
        return subprocess.run([sys.executable, 'tools/land'], cwd=self.lane, env=self.env | env,
                              capture_output=True, text=True)

    def main_log(self):
        return self.git(self.origin, 'log', '--format=%s', 'main').split('\n')[:-1]

    def test_a_push_rejected_because_main_moved_rebases_rebuilds_and_lands(self):
        landed = self.land(FAKE_OTHER_LANE=str(self.top / 'other'))
        self.assertEqual(landed.returncode, 0, landed.stdout + landed.stderr)
        self.assertIn('main moved during the checks; rebasing again', landed.stdout)
        # The retry rebuilds the rebased tree but does not lint again.
        self.assertEqual(landed.stdout.count('pass  workspace build'), 2)
        self.assertEqual(landed.stdout.count('pass  Clippy'), 1)
        self.assertEqual(self.main_log(), ['change', 'other', 'base'])
        [row] = [json.loads(line) for line in (self.lane / '.git/hirsel-landings.jsonl').read_text().splitlines()]
        self.assertEqual((row['branch'], row['landed']), ('my-lane', self.git(self.origin, 'rev-parse', 'main').strip()))

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


if __name__ == '__main__':
    unittest.main()
