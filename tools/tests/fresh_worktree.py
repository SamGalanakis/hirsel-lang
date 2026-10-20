#!/usr/bin/env python3
"""A fresh worktree of this tree has no generated build files: a bare Buck2
says to run sync.py, and after bootstrap it builds (tools/buck2/generated.py)."""
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest

REPO = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(REPO / 'tools/buck2'))
import generated  # noqa: E402

TARGET = '//:hirsel-span'


def git(*arguments, **kwargs):
    return subprocess.run(['git', *arguments], cwd=REPO, check=True, capture_output=True, text=True,
                          **kwargs).stdout.strip()


def snapshot():
    """A commit of this worktree as it is: its index, then every change and
    untracked file not ignored."""
    with tempfile.TemporaryDirectory() as scratch:
        index = Path(scratch) / 'index'
        shutil.copyfile(REPO / git('rev-parse', '--git-path', 'index'), index)
        env = os.environ | {'GIT_INDEX_FILE': str(index)}
        git('add', '-A', '.', env=env)
        tree = git('write-tree', env=env)
    return git('commit-tree', tree, '-p', 'HEAD', '-m', 'fresh worktree snapshot')


class FreshWorktree(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='hirsel-fresh-')
        self.addCleanup(self.temp.cleanup)
        self.tree = Path(self.temp.name) / 'tree'
        git('worktree', 'add', '-q', '--detach', str(self.tree), snapshot())
        self.addCleanup(git, 'worktree', 'remove', '--force', str(self.tree))
        self.addCleanup(self.stop_daemons)

    def buck2(self, *arguments):
        return subprocess.run([str(self.tree / '.buck2/bin/buck2'), *arguments], cwd=self.tree,
                              capture_output=True, text=True)

    def stop_daemons(self):
        if (self.tree / '.buck2/bin/buck2').exists():
            self.buck2('kill')
            self.buck2('--isolation-dir', 'buck2-bootstrap', 'kill')

    def script(self, name, *arguments):
        result = subprocess.run([sys.executable, str(self.tree / 'tools/buck2' / name), *arguments], cwd=self.tree,
                                capture_output=True, text=True, env={k: v for k, v in os.environ.items()
                                                                     if k != 'HIRSEL_LAND_TREE'})
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_bootstrap_makes_a_fresh_tree_build_and_a_bare_buck2_without_its_files_names_sync(self):
        self.assertFalse(any((self.tree / path).exists() for path in generated.BUILD_INPUTS))
        self.script('prepare.py')
        self.assertTrue(generated.current(self.tree))
        built = self.buck2('build', '-c', 'hirsel.priority=75', '--materializations', 'none', TARGET)
        self.assertEqual(built.returncode, 0, built.stderr[-4000:])
        self.assertEqual(subprocess.run(['git', 'status', '--porcelain'], cwd=self.tree, capture_output=True,
                                        text=True, check=True).stdout, '')
        for path in generated.BUILD_INPUTS:
            (self.tree / path).unlink()
        bare = self.buck2('uquery', TARGET)
        self.assertNotEqual(bare.returncode, 0)
        self.assertIn(f'missing: {generated.SYNC_HINT}', bare.stderr)
        self.script('sync.py', '--if-stale')
        self.assertEqual(self.buck2('uquery', TARGET).returncode, 0)

if __name__ == '__main__':
    unittest.main()
