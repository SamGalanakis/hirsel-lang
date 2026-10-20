#!/usr/bin/env python3
"""Tasks ratios retain throughput normalization and native recording selection."""
import copy
import json
from pathlib import Path
import runpy
import sys
import tempfile
import unittest

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
        frozen = json.loads((ROOT / 'bench/apps/tasks/results.json').read_text())
        template = next(r for r in frozen['samples'] if r['operation'] == 'list')
        rows = []
        for kind, count, instructions in [('hirsel', 10, 200), ('rust-one-worker', 5, 100),
                                           ('rust', 10, 50), ('hirsel-recorded', 10, 300)]:
            row = copy.deepcopy(template)
            row.update(kind=kind, completed=count, round=0, rate=16,
                       instructions_per_request=instructions / count)
            row['perf'] = {metric: instructions for metric in row['perf']}
            rows.append(row)
        document = dict(frozen, samples=rows, profiles={}, attribution={}, errors=[])
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / 'RESULTS.md'
            BENCH['report'](document, output)
            text = output.read_text()
        self.assertIn('| list / 16 | 1.000 [1.000, 1.000]', text)
        self.assertIn('| list/16 | 1.500 | 1.500 |', text)


if __name__ == '__main__':
    unittest.main()
