#!/usr/bin/env python3
"""Keep full-gate tool dependencies in Buck2's host test partition."""
import importlib.util
import contextlib
import io
from pathlib import Path
import sys
import tempfile
import unittest
import json
import subprocess
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'buck2'))
SPEC = importlib.util.spec_from_file_location('buck2_sync', Path(__file__).resolve().parents[1] / 'buck2/sync.py')
sync = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(sync)


def git(root, *arguments):
    subprocess.run(['git', *arguments], cwd=root, check=True, capture_output=True)


class GeneratedFiles(unittest.TestCase):
    """generated.py's stamp: the build inputs are current only for the tree they
    were generated from."""

    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        git(self.root, 'init', '-q', '-b', 'main')
        (self.root / '.gitignore').write_text(''.join(f'/{path}\n' for path in sync.generated.BUILD_INPUTS)
                                              + '/.buck2/\n')
        (self.root / 'Cargo.toml').write_text('[workspace]\n')
        git(self.root, 'add', '-A')
        git(self.root, '-c', 'user.name=t', '-c', 'user.email=t@e', 'commit', '-q', '-m', 'base')
        self.generate()

    def generate(self):
        state = sync.generated.tree_state(self.root)
        for path in sync.generated.BUILD_INPUTS:
            (self.root / path).parent.mkdir(parents=True, exist_ok=True)
            (self.root / path).write_text(path + '\n')
        sync.generated.record(self.root, state)

    def test_a_generated_tree_is_current_until_an_input_or_an_output_changes(self):
        current = lambda: sync.generated.current(self.root)  # noqa: E731
        self.assertTrue(current())
        for change, undo in [(lambda: (self.root / 'Cargo.toml').write_text('[workspace]\nmembers = []\n'),
                              lambda: git(self.root, 'checkout', '--', 'Cargo.toml')),
                             (lambda: (self.root / 'new.rs').write_text('fn f() {}\n'),
                              lambda: (self.root / 'new.rs').unlink()),
                             (lambda: (self.root / 'BUCK').write_text('edited\n'),
                              lambda: (self.root / 'BUCK').write_text('BUCK\n')),
                             (lambda: (self.root / 'third-party/BUCK').unlink(),
                              lambda: (self.root / 'third-party/BUCK').write_text('third-party/BUCK\n'))]:
            change()
            self.assertFalse(current())
            undo()
            self.assertTrue(current())
        # A commit moves HEAD's tree: the stamp names the old one.
        (self.root / 'Cargo.toml').write_text('[workspace]\nmembers = []\n')
        git(self.root, '-c', 'user.name=t', '-c', 'user.email=t@e', 'commit', '-q', '-am', 'next')
        self.assertFalse(current())
        self.generate()
        self.assertTrue(current())

    def test_a_lane_change_touching_a_generated_file_is_refused_with_the_commands_that_drop_it(self):
        self.assertEqual(sync.generated.refusal(['README.md', 'crates/a/src/lib.rs']), [])
        refused = '\n'.join(sync.generated.refusal(['docs/api.md', 'BUCK', 'README.md']))
        self.assertIn('no lane commits: BUCK, docs/api.md', refused)
        self.assertIn('git checkout $(git merge-base HEAD origin/main) -- docs/api.md', refused)
        self.assertIn('git rm -q --cached BUCK', refused)


class NativePoolRouting(unittest.TestCase):
    def test_no_campaign_implicitly_selects_the_host(self):
        self.assertTrue(sync.GATE_TESTS)
        for stage in sync.GATE_TESTS:
            self.assertFalse(stage.get('host', False), stage['name'])

    def test_filtered_native_entry_has_no_global_cover_dependency(self):
        stages = {stage['name']: stage for stage in sync.GATE_TESTS}
        self.assertNotIn('native_selection', stages['gate-native-full'])
        for index in range(1, sync.NATIVE_FULL_SHARDS + 1):
            self.assertEqual(stages[f'gate-native-full-shard-{index}']['native_selection'],
                             '//:native-selection')

    def test_generated_tests_have_no_host_partition(self):
        buck = (sync.ROOT / 'BUCK').read_text()
        self.assertNotIn('host_cases =', buck)
        self.assertNotIn('host = True', buck)


class GuardedRoots(unittest.TestCase):
    def test_a_root_or_a_path_module_installs_the_guard(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / 'plain.rs').write_text('//! A harness.\nuse std::fs;\n// hirsel_guard::install!();\n')
            (root / 'lib.rs').write_text('//! A library.\n#[cfg(test)]\nhirsel_guard::install!();\n')
            (root / 'support.rs').write_text('hirsel_guard::install!(Counting);\n')
            (root / 'main.rs').write_text('#[path = "support.rs"]\nmod support;\n')
            self.assertFalse(sync.guarded(root / 'plain.rs'))
            self.assertTrue(sync.guarded(root / 'lib.rs'))
            self.assertTrue(sync.guarded(root / 'main.rs'))


class SeededTests(unittest.TestCase):
    def test_a_binary_drawing_random_cases_anywhere_in_its_modules_is_seeded(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            main = root / 'main.rs'
            main.write_text('mod deep;\n')
            (root / 'deep').mkdir()
            (root / 'deep/mod.rs').write_text('#[test]\nfn fixed() {}\n')
            self.assertFalse(sync.seeded(root, main))
            (root / 'deep/mod.rs').write_text('proptest! {\n #[test]\n fn any(x in 0u8..) {}\n}\n')
            self.assertTrue(sync.seeded(root, main))

    def test_the_seed_reaches_exactly_the_proptest_binaries(self):
        import re
        buck = (Path(__file__).resolve().parents[2] / 'BUCK').read_text()
        seeded = {m[1] for m in re.finditer(r'hirsel_test\(\n    name = "([^"]+)",(.*?)\n\)', buck, re.S)
                  if 'seeded = True' in m[2]}
        self.assertIn('hirsel-parser-test-it', seeded)
        self.assertIn('hirsel-pipeline-tests-test-front', seeded)
        self.assertNotIn('hirsel-driver-test-it', seeded)


class ShardSizing(unittest.TestCase):
    def test_check_fails_on_cost_coverage_even_with_fresh_generated_files(self):
        with patch.object(sys, 'argv', ['sync.py', '--check']), \
             patch.object(sync, 'REINDEER', Path(__file__)), patch.object(sync.generated, 'current', return_value=True), \
             patch.object(sync, 'generate') as generate, \
             patch.object(sync, 'check_cost_coverage', return_value=['costs missing']), \
             contextlib.redirect_stderr(io.StringIO()) as errors:
            self.assertEqual(sync.main(), 1)
        self.assertIn('costs missing', errors.getvalue())
        generate.assert_not_called()

    def test_if_stale_generates_and_stamps_only_a_changed_tree(self):
        for fresh in (True, False):
            with self.subTest(fresh=fresh), patch.object(sys, 'argv', ['sync.py', '--if-stale']), \
                 patch.object(sync, 'REINDEER', Path(__file__)), \
                 patch.object(sync.generated, 'current', return_value=fresh), \
                 patch.object(sync.generated, 'tree_state', return_value='state'), \
                 patch.object(sync.generated, 'record') as record, patch.object(sync, 'generate') as generate:
                self.assertEqual(sync.main(), 0)
                self.assertEqual((generate.called, record.called), (not fresh, not fresh))

    def test_granted_cpus_determine_shards(self):
        with tempfile.TemporaryDirectory() as scratch:
            here = Path(scratch)
            binary = sync.SHARDED[0]
            (here / 'test-costs').mkdir()
            (here / f'test-costs/{binary}.tsv').write_text('case\t640.0\n')
            for cpus, expected in [(1, 16), (2, 8), (4, 4)]:
                (here / 'test-sizes.json').write_text(json.dumps({binary: {'cpu_count': cpus}}))
                with patch.object(sync, 'HERE', here), patch.object(sync, 'COSTS', here / 'test-costs'):
                    self.assertEqual(sync.shards(binary)[0], expected)

    def test_exactly_five_percent_missing_is_allowed_but_stale_rows_do_not_help(self):
        with tempfile.TemporaryDirectory() as scratch, patch.object(sync, 'SHARDED', ['binary']), \
             patch.object(sync, 'REQUIRED_COSTS', {'binary'}):
            table = Path(scratch) / 'binary.tsv'
            table.write_text(''.join(f'case{i}\t1\n' for i in range(19)) + 'deleted\t10000\n')
            with patch.object(sync, 'COSTS', Path(scratch)):
                self.assertEqual(sync.cost_coverage({'binary': {f'case{i}' for i in range(20)}}), [])
                errors = sync.cost_coverage({'binary': {f'case{i}' for i in range(21)}})
                self.assertIn('2/21 cases', errors[0])
                self.assertIn('case20', errors[0])
                self.assertIn('no case inventory', sync.cost_coverage({})[0])

    def test_unrefreshed_binary_warns_without_blocking_the_gate(self):
        binary = 'hirsel-rt-test-it'
        with patch.object(sync, 'SHARDED', [binary]), contextlib.redirect_stderr(io.StringIO()) as output:
            self.assertEqual(sync.cost_coverage({binary: {'new::case'}}), [])
        self.assertIn(f'warning: {binary}: 1/1 cases', output.getvalue())

    def test_listing_is_on_the_pool_and_never_runs_test_bodies(self):
        from subprocess import CompletedProcess
        binary = sync.SHARDED[0]
        output = (f'✓ Pass: root//:{binary}-shard-1 (0.1s)\n'
                  'Stdout:\nmacro_generated::a: test\nignored::b: test\n')
        with patch.object(sync, 'SHARDED', [binary]), patch('test_costs.configured_shards', return_value=2), \
             patch.object(sync.subprocess, 'run', return_value=CompletedProcess([], 0, output)) as run, \
             patch.object(sync, 'cost_coverage', return_value=[]) as coverage:
            self.assertEqual(sync.check_cost_coverage(), [])
            coverage.assert_called_once_with({binary: {'macro_generated::a', 'ignored::b'}})
            command = run.call_args.args[0]
            self.assertIn('hirsel.tests=focus', command)
            self.assertIn('--test-arg=--list', command)
            self.assertIn('--test-arg=--include-ignored', command)
            self.assertIn('--test-arg=', command)
            self.assertNotIn('hirsel.tests=host', command)
        with patch.object(sync.subprocess, 'run', return_value=CompletedProcess([], 1, 'pool error')):
            self.assertIn('pool error', sync.check_cost_coverage()[0])


if __name__ == '__main__':
    unittest.main()
