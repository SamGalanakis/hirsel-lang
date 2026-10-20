#!/usr/bin/env python3
"""Counted micro sessions: counter validity, paired statistics and execution boundaries."""
import importlib.machinery
import importlib.util
import json
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / 'bench/micro'))
import measure  # noqa: E402

loader = importlib.machinery.SourceFileLoader('micro_run', str(ROOT / 'bench/micro/run'))
run = importlib.util.module_from_spec(importlib.util.spec_from_loader(loader.name, loader))
loader.exec_module(run)


class Counters(unittest.TestCase):

    def test_unavailable_or_partial_measurements_fail_closed(self):
        for value in ('<not supported>', '<not counted>', 'nan', 'inf', '0'):
            with self.subTest(value=value), self.assertRaises(ValueError):
                measure.perf_counts(f'{value};;instructions:u;', ('instructions:u',))
        with self.assertRaises(ValueError):
            measure.perf_counts('')

    def test_sibling_delta_excludes_idle_and_iowait(self):
        before = {'cpu15': {'total': 100, 'idle': 30}}
        after = {'cpu15': {'total': 200, 'idle': 100}}
        self.assertEqual(measure.busy_share(before, after), {'cpu15': 0.3})
        self.assertEqual(measure.busy_share(before, before), {'cpu15': None})
        self.assertEqual(measure.cpu_list('0-2,7,15-16'), [0, 1, 2, 7, 15, 16])


class Sessions(unittest.TestCase):
    def test_seeded_schedule_interleaves_every_participant_once_per_round(self):
        order = measure.schedule(['old', 'new', 'rust'], 10, 42)
        self.assertEqual(order, measure.schedule(['old', 'new', 'rust'], 10, 42))
        self.assertNotEqual(order, measure.schedule(['old', 'new', 'rust'], 10, 43))
        for n in range(10):
            self.assertEqual({name for r, name in order if r == n}, {'old', 'new', 'rust'})

    def test_paired_bootstrap_preserves_shared_round_variation(self):
        self.assertEqual(measure.bootstrap_ratio([2, 20, 200], [1, 10, 100], 0), [2, 2])
        with self.assertRaises(ValueError):
            measure.bootstrap_ratio([1], [], 0)

    def test_saved_sessions_expose_only_instruction_ratios(self):
        def session(multiplier):
            return {'participants': [{'name': 'rust'}], 'samples': [
                {'name': 'rust', 'counts': dict.fromkeys(measure.EVENTS, multiplier)}]}
        rows = measure.cross_session(session(2), session(1))
        self.assertEqual({r['metric'] for r in rows}, {'instructions', 'instructions:u'})
        self.assertTrue(all(r['ratio'] == 2 for r in rows))

    def test_ratio_table_uses_reference_counts_and_preserves_round_pairs(self):
        data = {'participants': [{'name': 'rust'}, {'name': 'hirsel'}], 'seed': 0, 'samples': []}
        for n in (2, 0, 1):
            for name, mult in (('hirsel', 2), ('rust', 1)):
                counts = dict.fromkeys(measure.EVENTS, mult * (n + 1))
                counts['instructions'] *= 3
                data['samples'].append({'round': n, 'name': name, 'counts': counts})
        own, other = measure.comparisons(data, 'rust')
        self.assertEqual(own['instructions_ratio'], 1)
        self.assertEqual(other['instructions_ratio'], 2)
        self.assertEqual(other['cycles_ratio'], 2)
        self.assertEqual(other['ipc'], 3)
        self.assertEqual(other['task-clock_ratio_ci95'], [2, 2])


    def test_failed_saved_sessions_are_rejected_before_running(self):
        with tempfile.TemporaryDirectory() as scratch:
            entries = Path(scratch) / 'participants.json'
            entries.write_text(json.dumps([{'name': name, 'command': ['/bin/true']} for name in ('a', 'b')]))
            previous = Path(scratch) / 'failed.json'
            previous.write_text(json.dumps({'schema': 1, 'suite': 'micro', 'status': 'failed'}))
            with patch.object(sys, 'argv', ['run', str(entries), '--output', str(Path(scratch) / 'out.json'),
                                           '--compare', str(previous), '--cpu', str(min(__import__('os').sched_getaffinity(0)))]), \
                    patch.object(run, 'sample') as sample:
                self.assertEqual(run.main(), 2)
            sample.assert_not_called()

    def test_output_mismatch_prevents_a_verdict(self):
        with tempfile.TemporaryDirectory() as scratch:
            participants = Path(scratch) / 'participants.json'
            participants.write_text(json.dumps([{'name': name, 'command': ['/bin/true']} for name in ('a', 'b')]))
            output = Path(scratch) / 'result.json'
            with patch.object(sys, 'argv', ['run', str(participants), '--output', str(output), '--cpu', str(min(__import__('os').sched_getaffinity(0)))]), \
                    patch.object(run, 'timing_lock', return_value=open('/dev/null')), \
                    patch.object(run, 'sample', side_effect=lambda p, *_: {'stdout_sha256': p['name']}):
                self.assertEqual(run.main(), 2)
            failure = json.loads(output.read_text())
            self.assertEqual(failure['status'], 'failed')
            self.assertNotIn('comparisons', failure)
            self.assertEqual(len(failure['samples']), 20)


if __name__ == '__main__':
    unittest.main()
