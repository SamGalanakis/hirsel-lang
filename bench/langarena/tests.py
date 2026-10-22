#!/usr/bin/env python3
"""Focused native protocol and comparison tests; no reference languages run."""
import json
import os
from pathlib import Path
import re
import subprocess
import tempfile
import unittest
from unittest.mock import patch

import build
import compare

ROOT = build.ROOT
HERE = build.HERE


class Comparison(unittest.TestCase):
    def test_requires_task_specific_checksum_line(self):
        self.assertEqual(compare.parse_result('X: OK in 1.234s\n', 'X'), 1.234)
        for output in ['X: ERR[actual=1, expected=2] in 1.0s', 'Y: OK in 1.0s', 'Summary: 1s']:
            with self.assertRaises(ValueError):
                compare.parse_result(output, 'X')

    def test_failed_or_incomplete_rounds_are_not_summarized(self):
        entry = dict(samples=[dict(status='OK', seconds=1), dict(status='failed', seconds=2)])
        self.assertIsNone(compare.aggregate(entry, 'seconds', 2))
        self.assertIsNone(compare.aggregate(dict(samples=entry['samples'][:1]), 'seconds', 2))
        self.assertEqual(compare.aggregate(dict(samples=[dict(status='OK', seconds=x) for x in [1, 9, 2]]), 'seconds', 3), 2)

    def test_estimates_use_rust_ratio_and_refuse_missing_anchor(self):
        self.assertEqual(compare.published_estimate(2, 4), 8)
        for value in [None, 0, -1]:
            self.assertIsNone(compare.published_estimate(2, value))
        self.assertIsNone(compare.published_estimate(None, 4))
        self.assertEqual(compare.placement(3, {'Rust': 2, 'C': 4}), '2 / 3')

    def test_file_identity_matches_sha256_and_detects_a_change(self):
        with tempfile.TemporaryDirectory() as directory:
            binary = Path(directory) / 'program'
            binary.write_bytes(b'first')
            before = compare.identity(binary)
            self.assertEqual(before['sha256'], compare.sha(binary))
            binary.write_bytes(b'other')
            self.assertNotEqual(before, compare.identity(binary))

    def test_report_keeps_debug_results_separate_from_release_ratios(self):
        source = dict(bytes=1, gzip_bytes=1, lines=1)
        participants = {name: dict(status='ready', cold=cold, incremental=cold / 2,
            source=source, compiler=dict(sha256='compiler'), binary_identity=dict(sha256=name))
            for name, cold in [('hirsel', 10), ('rust', 5), ('cranelift', 3)]}
        row = {name: dict(status='ready', source=source,
            samples=[dict(status='OK', seconds=seconds, rss_kib=1024)])
            for name, seconds in [('hirsel', 2), ('rust', 1), ('cranelift', 9)]}
        data = dict(rounds=1, tasks={'X': row}, config='run.js', seed=1, at='now', host='host',
            published_file='results.js', published_sha256='published', cpu=31, timing_lock='lock',
            consumed_sources=dict(sha256='sources'), perf='unavailable', participants=participants,
            invocation='compare.py --debug', session='example')
        published = dict(runs={'Rust': 'prod'}, **{'X-runtime': {'Rust': 1}})
        with tempfile.TemporaryDirectory() as directory:
            report = Path(directory) / 'results.md'
            compare.report(data, published, report)
            text = report.read_text()
        self.assertIn('Cranelift debug status | Debug s | Debug MiB | Debug/Rust time |', text)
        self.assertIn('OK (1/1) | 9.000000000 | 1.000 | 9.000 |', text)
        self.assertIn('| seconds | 2.000 | 1 / 50 |', text)
        self.assertIn('| cold | 10.000 | 5.000 | 3.000 |', text)

    def test_new_group_is_discovered_and_duplicate_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            here = Path(directory)
            (here / 'run.js').write_text('[{"name":"Etc::Sieve"}]')
            (here / 'sieve.hir').write_text('// langarena: Etc::Sieve\n')
            with patch.object(build, 'HERE', here):
                self.assertEqual(build.groups(), {'sieve': ['Etc::Sieve']})
                (here / 'other.hir').write_text('// langarena: Etc::Sieve\n')
                with self.assertRaisesRegex(ValueError, 'duplicate'):
                    build.groups()


class NativeProtocol(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.scratch = tempfile.TemporaryDirectory(prefix='langarena-protocol-')
        cls.directory = Path(cls.scratch.name)
        cls.binary = cls.directory / 'langarena'
        cli = os.environ.get('HIRSEL') or str(ROOT / 'buck-out/tmp/cargo-bin-exe/gate-langarena/hirsel')
        manifest = build.generate()
        env = os.environ | {'XDG_CACHE_HOME': str(cls.directory / 'cache')}
        subprocess.run([cli, 'build', '--release', manifest, '-o', cls.binary], env=env, check=True)
        cls.grants = cls.directory / 'grants.json'
        cls.grants.write_text(json.dumps(dict(io=dict(stdout='stdout.bytes@1',
            monotonic_clock='clock.monotonic.ns@1', wall_clock='clock.wall.unix_ns@1',
            dirs=[dict(path='', host=str(cls.directory), access='read')]), args=[])))

    @classmethod
    def tearDownClass(cls):
        cls.scratch.cleanup()

    def run_config(self, tasks, name=''):
        config = self.directory / 'tasks.json'
        config.write_text(json.dumps(tasks))
        return subprocess.run([self.binary, '--hirsel-grants', self.grants, 'tasks.json', name],
                              cwd=self.directory, capture_output=True, text=True)

    def test_all_implemented_quick_checksums(self):
        tasks = json.loads((HERE / 'test.js').read_text())
        output = self.run_config(tasks)
        names = {name for group in build.groups().values() for name in group}
        self.assertEqual(output.returncode, 0, output.stdout + output.stderr)
        for name in names:
            self.assertIn(name + ': OK in ', output.stdout)
        self.assertIn(f', {len(names)}, {len(names)}, 0\n', output.stdout)
        self.assertRegex(output.stdout, r'^start: [1-9][0-9]+\n')

    def test_explicit_warmup_accumulates_and_filter_is_case_insensitive(self):
        # At depth 1, the tree sum is 3. Five warmup + two timed iterations = 21.
        task = dict(name='Binarytrees::Obj', depth=1, warmup_iterations=5, iterations=2, checksum=21)
        other = dict(name='Binarytrees::Arena', depth=1, iterations=1, checksum=0)
        output = self.run_config([task, other], 'oBj')
        self.assertEqual(output.returncode, 0, output.stdout + output.stderr)
        self.assertIn('Binarytrees::Obj: OK', output.stdout)
        self.assertNotIn('Binarytrees::Arena:', output.stdout)

    def test_zero_warmup_does_not_run_an_extra_iteration(self):
        task = dict(name='Binarytrees::Arena', depth=1, warmup_iterations=0, iterations=2, checksum=6)
        output = self.run_config([task])
        self.assertEqual(output.returncode, 0, output.stdout + output.stderr)

    def test_checksum_failure_exits_nonzero(self):
        task = dict(name='Binarytrees::Obj', depth=1, iterations=2, checksum=0)
        output = self.run_config([task])
        self.assertNotEqual(output.returncode, 0)
        self.assertIn('ERR[actual=9, expected=0]', output.stdout)
        self.assertIn(', 1, 0, 1\n', output.stdout)

    def test_no_match_is_an_error(self):
        output = self.run_config(json.loads((HERE / 'test.js').read_text()), 'not-a-task')
        self.assertNotEqual(output.returncode, 0)
        self.assertIn('no implemented tasks selected', output.stderr)


if __name__ == '__main__':
    unittest.main(verbosity=2)
