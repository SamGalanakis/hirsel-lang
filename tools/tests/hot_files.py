#!/usr/bin/env python3
"""Concurrent independent registrations survive real Git rebases without configuration."""
from pathlib import Path
import os
import json
from types import SimpleNamespace
from unittest.mock import patch
import runpy
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
CHECK = runpy.run_path(str(ROOT / 'tools/check-hot-files'))['check']


class HotFiles(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(prefix='hot-files-test-')
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.env = {k: v for k, v in os.environ.items() if not k.startswith('GIT_')} | dict(
            GIT_CONFIG_NOSYSTEM='1', GIT_CONFIG_GLOBAL='/dev/null',
            GIT_AUTHOR_NAME='test', GIT_AUTHOR_EMAIL='test@example.org',
            GIT_COMMITTER_NAME='test', GIT_COMMITTER_EMAIL='test@example.org', GIT_EDITOR='true')
        self.git('init', '-q', '-b', 'main')
        self.registry = self.root / 'crates/example/tests/it/modules.rs'
        self.registry.parent.mkdir(parents=True)
        self.registry.write_text('mod first;\n')
        self.registry.with_name('main.rs').write_text('include!("modules.rs");\n')
        (self.root / 'tools').mkdir()
        self.ledger = self.root / 'tools/known-failures.tsv'
        self.ledger.write_text('first\treason\towner\t2026-10-22\n')
        # The checker uses main-runs' existing ledger parser.
        (self.root / 'tools/main-runs').symlink_to(ROOT / 'tools/main-runs')
        (self.root / '.gitattributes').write_text((ROOT / '.gitattributes').read_text())
        self.commit('base')

    def git(self, *args):
        return subprocess.run(['git', *args], cwd=self.root, env=self.env, check=True,
                              capture_output=True, text=True).stdout

    def commit(self, message):
        self.git('add', '-A')
        self.git('commit', '-qm', message)

    def lane(self, name, row_id=None):
        self.git('checkout', '-qb', name, 'main')
        with self.registry.open('a') as out:
            out.write(f'mod {name};\n')
        self.registry.with_name(name + '.rs').write_text('#[test]\nfn passes() {}\n')
        with self.ledger.open('a') as out:
            out.write(f'{row_id or name}\treason\towner\t2026-10-22\n')
        self.commit(name)

    def test_two_branches_rebase_in_both_orders_without_a_merge_driver(self):
        self.lane('alpha')
        self.lane('beta')
        for first, second in [('alpha', 'beta'), ('beta', 'alpha')]:
            self.git('checkout', '-q', '-B', 'proof', second)
            self.git('rebase', first)
            self.assertEqual(set(self.registry.read_text().splitlines()),
                             {'mod first;', 'mod alpha;', 'mod beta;'})
            self.assertEqual(CHECK(self.root), [])

    def test_duplicate_declarations_and_ledger_identities_are_rejected(self):
        with self.registry.open('a') as out:
            out.write('mod first;\n')
        with self.ledger.open('a') as out:
            out.write('first\tanother reason\tother owner\t2026-10-22\n')
        errors = '\n'.join(CHECK(self.root))
        self.assertIn('duplicate line', errors)
        self.assertIn('duplicate identity', errors)

    def test_formatting_checks_module_bodies_without_sorting_union_declarations(self):
        self.registry.write_text('mod zed;\nmod alpha;\n')
        for name in ['zed', 'alpha']:
            self.registry.with_name(name + '.rs').write_text('#[test]\nfn passes() {}\n')
        fmt = runpy.run_path(str(ROOT / 'tools/buck2/fmt.py'))['main']
        query = {'example': {'crate_root': 'crates/example/tests/it/main.rs', 'edition': '2024'}}
        with patch.dict(fmt.__globals__, ROOT=self.root), \
                patch('subprocess.run', side_effect=[SimpleNamespace(stdout=json.dumps(query)),
                                                     SimpleNamespace(returncode=0)]) as run:
            self.assertEqual(fmt(), 0)
        command = run.call_args_list[1].args[0]
        self.assertIn('crates/example/tests/it/zed.rs', command)
        self.assertIn('crates/example/tests/it/alpha.rs', command)
        self.assertNotIn('crates/example/tests/it/modules.rs', command)

    def test_attributes_cannot_enter_a_union_registry(self):
        self.registry.write_text('#[cfg(unix)]\nmod first;\n')
        self.assertIn('expected one independent mod declaration', '\n'.join(CHECK(self.root)))


if __name__ == '__main__':
    unittest.main()
