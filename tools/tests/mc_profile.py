#!/usr/bin/env python3
"""Small DAGs with independently calculable schedules and invalid traces."""
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

spec = importlib.util.spec_from_file_location(
    'mc_profile', Path(__file__).resolve().parents[1] / 'mc-profile.py')
mc = importlib.util.module_from_spec(spec)
spec.loader.exec_module(mc)


def row(task, ns, after):
    return dict(task=task, ns=ns, after=after, operation='test')


class Model(unittest.TestCase):
    def test_spawn_parallel_children_and_join_dependency(self):
        rows = [row([], 10, 0), row([0], 100, 1), row([1], 100, 1), row([], 10, 3)]
        # 1: root 0..10, commit 10..20; children 20..120 and 120..220;
        # commits 120..130 and 220..230; root 230..240, commit 240..250.
        self.assertEqual(mc.simulate(rows, 1, 10)['makespan_ns'], 250)
        # 2: children both 20..120; commits 120..130..140; root to 160.
        result = mc.simulate(rows, 2, 10)
        self.assertEqual(result['makespan_ns'], 160)
        self.assertEqual(result['commit_busy_share'], 40 / 160)
        self.assertEqual(result['stall_share'], 120 / 160)
        self.assertEqual(mc.bounds(rows, 10), dict(work_ns=260, span_ns=160, work_span=260/160))

    def test_long_head_and_worker_reuse_before_commit(self):
        rows = [row([], 0, 0), row([0], 100, 1), row([1], 10, 1), row([2], 10, 1)]
        result = mc.simulate(rows, 2, 10)
        self.assertEqual(result['makespan_ns'], 140)
        self.assertEqual(result['hol_ready_share'], 90 / 140)

    def test_fifo_head_can_prevent_parent_from_spawning_next_child(self):
        rows = [row([], 0, 0), row([0], 100, 1), row([], 0, 1),
                row([1], 100, 3), row([], 0, 4)]
        # The parent's second spawn segment can finish early, but its commit
        # stays behind the first child's long segment. More workers cannot help.
        self.assertEqual(mc.simulate(rows, 1, 10)['makespan_ns'], 250)
        self.assertEqual(mc.simulate(rows, 16, 10)['makespan_ns'], 250)

    def test_serial_task_does_not_gain_parallelism(self):
        rows = [row([], 100, i) for i in range(4)]
        self.assertEqual(mc.simulate(rows, 16, 10)['makespan_ns'], 440)

    def test_zero_cost_and_zero_length(self):
        self.assertEqual(mc.simulate([row([], 0, 0)], 1, 0)['makespan_ns'], 0)
        self.assertEqual(mc.simulate([row([], 10, 0)], 1, 0)['stall_share'], 1)

    def test_reader_rejects_future_and_missing_task_dependencies(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'trace'
            for rows in ([row([], 10, 1)], [row([0], 10, 0)],
                         [row([], 10, 0), row([], 10, 0)], [row([], -1, 0)]):
                path.write_text('mc-profile 1\n' + ''.join('mc ' + json.dumps(r) + '\n' for r in rows))
                with self.assertRaises(ValueError):
                    mc.read_log(path)
            rows = [row([], 10, 0), row([0], 10, 1)]
            path.write_text('hirsel-measurement 1\nmc-profile 1\n' + ''.join('mc ' + json.dumps(r) + '\n' for r in rows))
            self.assertEqual(mc.read_log(path), rows)


if __name__ == '__main__':
    unittest.main()
