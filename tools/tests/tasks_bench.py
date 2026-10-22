#!/usr/bin/env python3
"""Tasks ratios retain throughput normalization and native recording selection."""
import json
from pathlib import Path
import runpy
import sys
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / 'bench/apps/tasks'))
BENCH = runpy.run_path(str(ROOT / 'bench/apps/tasks/bench'))


class TasksBench(unittest.TestCase):
    def test_operation_counters_are_normalized_but_fanout_is_per_mutation(self):
        value = {'operation': 'list', 'completed': 4, 'perf': {'instructions': 100}}
        self.assertEqual(BENCH['per_request'](value, 'instructions'), 25)
        for operation in ('cold', 'restart', 'sse'):
            value['operation'] = operation
            self.assertEqual(BENCH['per_request'](value, 'instructions'), 100)

    def test_profile_separates_detaches_from_explicit_copies_and_rc(self):
        profile = "  2.00%  [.] hirsel_rt_v2_dup\n  3.00%  [.] hirsel_rt_v2_release\n  4.00%  [.] hirsel_rt_v2_detach\n  5.00%  [.] copy_graph\n  6.00%  [.] drop_last\n"
        costs = BENCH['profile_costs'](profile)
        self.assertEqual(list(costs.values()), [2, 3, 6, 4, 5, 0])

    def test_recorded_participant_uses_the_same_native_binary(self):
        self.assertTrue(BENCH['is_hirsel']('hirsel-recorded'))
        self.assertTrue(BENCH['is_hirsel']('hirsel'))
        self.assertFalse(BENCH['is_hirsel']('rust-one-worker'))

    def test_report_compares_per_request_against_one_worker_and_recording_against_plain(self):
        template = dict(operation='list', first_ms=1, idle_rss=1024, load_rss=2048,
                        binary_bytes=4096, requests_per_s=16, cpu_s_per_request=.001,
                        p50_us=100, p99_us=200, p999_us=300, max_us=400,
                        sibling_busy_share=0,
                        perf={metric: 100 for metric in
                              ('instructions', 'instructions:u', 'cycles', 'cycles:u', 'task-clock')})
        rows = []
        for kind, count, instructions in [('hirsel', 10, 200), ('rust-one-worker', 5, 100),
                                           ('rust', 10, 50), ('hirsel-recorded', 10, 300)]:
            row = dict(template)
            row.update(kind=kind, completed=count, round=0, rate=16,
                       instructions_per_request=instructions / count)
            row['perf'] = {metric: instructions for metric in row['perf']}
            rows.append(row)
        document = dict(session='test', git='test', seed=42, rounds=1, rates=[16],
                        seconds=1, samples=rows, profiles={}, attribution={}, errors=[])
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / 'RESULTS.md'
            BENCH['report'](document, output)
            text = output.read_text()
        self.assertIn('| list / 16 | 1.000 [1.000, 1.000]', text)
        self.assertIn('| list/16 | 1.500 | 1.500 |', text)

    def test_report_only_reads_and_writes_the_selected_external_session(self):
        document = dict(session='test', git='test', seed=42, rounds=1, rates=[],
                        seconds=1, samples=[], profiles={}, errors=[],
                        binaries={'hirsel': 'current'}, attribution={'cold': 'Saved interpretation'})
        for notes, expected in [(None, document['attribution']),
                                (dict(binaries={'hirsel': 'old'}, notes={'cold': 'Stale'}), {})]:
            with self.subTest(notes=notes), tempfile.TemporaryDirectory() as directory:
                output = Path(directory)
                (output / 'results.json').write_text(json.dumps(document))
                if notes is not None:
                    (output / 'attribution.json').write_text(json.dumps(notes))
                with patch.object(sys, 'argv', ['bench', '--report-only', '--output-dir', directory]), \
                        patch('os.sched_setaffinity'):
                    self.assertEqual(BENCH['main'](), 0)
                saved = json.loads((output / 'results.json').read_text())
                self.assertEqual(saved['attribution'], expected)
                self.assertEqual(saved['samples'], [])
                self.assertIn('Session: test', (output / 'RESULTS.md').read_text())


if __name__ == '__main__':
    unittest.main()
