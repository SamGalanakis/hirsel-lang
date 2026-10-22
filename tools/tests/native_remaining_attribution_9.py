#!/usr/bin/env python3
"""Fail-closed profile partition and inherited one-body experiment contracts."""
import copy
import importlib.machinery
import importlib.util
from pathlib import Path
import tempfile
import unittest
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[2]
loader = importlib.machinery.SourceFileLoader('remaining9', str(ROOT/'tools/compiler-bench/native-remaining-attribution-9'))
spec = importlib.util.spec_from_loader(loader.name, loader)
h = importlib.util.module_from_spec(spec)
loader.exec_module(h)


def sample(comm='hirsel-compiler', pid=1, leaf='work', callers=(), period=100):
    return {'comm': comm, 'pid': pid, 'period': period,
            'frames': [(name, 'hirsel') for name in (leaf, *callers)]}


class Attribution(unittest.TestCase):
    def test_lost_capture_and_startup_classification(self):
        h.validate_record('[ perf record: Captured and wrote 42 MB (10 samples) ]')
        for text in ('', 'Captured and wrote 42 MB; lost 25 chunks', 'Captured and wrote 42 MB; lost 1 samples'):
            with self.assertRaises(ValueError):
                h.validate_record(text)
        self.assertEqual(h.cause(sample(callers=('hirsel_engine::registry::build::registry',))), 'builtin-registry-startup')
        self.assertEqual(h.cause(sample(callers=('native::adapt::manifest',))), 'native-manifest')

    def test_cli_rejects_lost_samples_and_changed_script(self):
        with tempfile.TemporaryDirectory() as temp:
            path = Path(temp)/'samples'
            path.write_text('hirsel-compiler 1/1 100 instructions:\n\t123 work (hirsel)\n')
            for lost, digest in ((1, h.base.sha(path)), (0, 'incorrect')):
                done = subprocess.run([sys.executable, str(ROOT/'tools/compiler-bench/native-remaining-attribution-9'),
                    'analyze', str(path), '--lost-samples', str(lost), '--script-sha256', digest], capture_output=True)
                self.assertNotEqual(done.returncode, 0)
                self.assertIn(b'lost samples or changed profile', done.stderr)

    def test_disjoint_weighted_partition_and_original_denominator(self):
        rows = [sample(leaf='mi_malloc_aligned', callers=('hirsel_validate::input::digest',)),
                sample(leaf='hirsel_validate::input::digest', period=300),
                sample(leaf='sha2::sha256::digest_blocks', period=500),
                sample('ld.lld', pid=2, period=200), sample('llvm-objcopy', pid=3),
                sample('llvm-objcopy', pid=4)]
        result = h.partition(rows, {'ld.lld': 1, 'llvm-objcopy': 2}, True, 1300)
        self.assertAlmostEqual(sum(r['percent'] for r in result['causes']), 100)
        self.assertAlmostEqual(sum(r.get('original_denominator_points', 0) for r in result['causes']), 39.93)
        self.assertAlmostEqual(sum(r['estimated_instructions'] for r in result['causes']), 1300)
        by_cause = {r['cause']: r for r in result['causes']}
        self.assertEqual(by_cause['allocator-leaf']['period'], 100)
        self.assertEqual(by_cause['validation-input-digest']['period'], 300)

    def test_nested_sha_and_v_are_not_added_twice(self):
        row = sample(leaf='sha2::sha256::digest_blocks', callers=(
            'hirsel_validate::input::digest', 'hirsel_validate::validate'))
        self.assertEqual(h.cause(row, True), 'prior:sha-leaf')
        self.assertEqual(h.cause(row), 'validation-input-digest')
        row['frames'][0] = ('__memcpy', '[kernel.kallsyms]')
        self.assertEqual(h.cause(row), 'kernel')

    def test_view_check_receipt_and_full_v_are_separate(self):
        self.assertEqual(h.cause(sample(callers=('hirsel_validate::input::check_view',
            '::verdict_from', '::validation_state'))), 'signature-view-check')
        self.assertEqual(h.cause(sample(callers=('::verdict_from',))), 'receipt-reconstruction')
        self.assertEqual(h.cause(sample(callers=('hirsel_validate::validate',))), 'full-V-check')
        self.assertEqual(h.cause(sample(leaf='0x1234')), 'unknown-symbol')
        self.assertEqual(h.cause(sample(leaf='unrecognized')), 'unknown-other')

    def test_missing_and_duplicate_descendants_fail_closed(self):
        rows = [sample(), sample('ld.lld', 2), sample('llvm-objcopy', 3), sample('llvm-objcopy', 3)]
        with self.assertRaisesRegex(ValueError, 'distinct descendant'):
            h.partition(rows, {'hirsel-compiler': 1, 'ld.lld': 1, 'llvm-objcopy': 2})
        with self.assertRaisesRegex(ValueError, 'missing compiler'):
            h.partition([], {'hirsel-compiler': 1})
        with self.assertRaisesRegex(ValueError, 'missing historical residual'):
            h.partition([sample(leaf='sha2::sha256::digest_blocks')], {'hirsel-compiler': 1}, True)

    def test_parser_preserves_pid_period_and_rejects_missing_stacks(self):
        rows = h.samples('hirsel-compiler 10/11 200 instructions: \n\t123 work (hirsel)\n')
        self.assertEqual(rows[0]['period'], 200)
        self.assertEqual(rows[0]['pid'], 10)
        for text in ('', 'garbage', 'hirsel 1/1 0 instructions:\n\t123 work (hirsel)',
                     'hirsel 1/1 42 instructions:'):
            with self.assertRaises(ValueError):
                h.samples(text)

    def test_changed_inputs_failed_missing_or_zero_selected_sessions(self):
        with tempfile.TemporaryDirectory() as temp:
            path = Path(temp)/'input'
            path.write_text('frozen')
            frozen = h.base.inventory([path])
            good = {'status': 'pass', 'frozen_verified': True, 'rounds': 1, 'attempts': [
                {'edit': 'body', 'telemetry': True, 'status': 'pass',
                 'perf': {'running_percent': {'instructions': 100}},
                 'timings': {'counts': {'object misses': 1, 'V runs': 1}}}]}
            h.validate_session(good, frozen, [path])
            for key, value in (('status', 'failed'), ('frozen_verified', False), ('attempts', []), ('rounds', 10)):
                bad = copy.deepcopy(good)
                bad[key] = value
                with self.assertRaises(ValueError):
                    h.validate_session(bad, frozen, [path])
            for change in ({'perf': None}, {'status': 'failed'},
                           {'perf': {'running_percent': {'instructions': 90}}},
                           {'timings': {'counts': {'object misses': 0, 'V runs': 0}}}):
                bad = copy.deepcopy(good)
                bad['attempts'][0].update(change)
                with self.assertRaises(ValueError):
                    h.validate_session(bad, frozen, [path])
            path.write_text('changed')
            with self.assertRaises(RuntimeError):
                h.validate_session(good, frozen, [path])


if __name__ == '__main__':
    unittest.main()
