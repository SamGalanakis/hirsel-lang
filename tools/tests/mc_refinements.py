"""Hand-computable M0b queues and the eligibility/pending-message boundaries."""
import importlib.util
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location('refinements', Path(__file__).resolve().parents[1]/'mc-refinements.py')
m = importlib.util.module_from_spec(spec)
spec.loader.exec_module(m)


class Model(unittest.TestCase):
    def test_independent_early_op_enables_continuation(self):
        rows = m.scenario(tasks=2, rounds=2, segment_ns=100)
        strict = m.simulate(rows, 2, commit_ns=10)
        early = m.simulate(rows, 2, 'early', commit_ns=10)
        self.assertEqual(strict['makespan_ns'], 930)
        self.assertEqual(early['makespan_ns'], 840)
        self.assertEqual(early['early_commits'], 1)
        self.assertEqual(early['max_pending'], 1)
        self.assertEqual(early['publish'][0], (1, 820))

    def test_rule_one_bounds_each_task_to_one_early_turn_per_head(self):
        rows = m.scenario(tasks=8, rounds=5)
        r = m.simulate(rows, 8, 'early')
        for head in {h for h, _ in r['early_history']}:
            tasks = [rows[i]['task'] for h, i in r['early_history'] if h == head]
            self.assertEqual(len(tasks), len(set(tasks)))
        self.assertEqual(sum(h == 0 for h, _ in r['early_history']), 7)

    def test_shared_object_refuses_early_commits(self):
        rows = m.scenario(shared=True)
        self.assertEqual(m.simulate(rows, 8, 'early')['early_commits'], 0)

    def test_earlier_possible_read_conflicts_with_write(self):
        rows = m.scenario(tasks=2, rounds=2)
        # Models a captured Sender or a handle obtainable through type closure.
        rows[0]['possible']['read'] = ['1']
        self.assertEqual(m.simulate(rows, 8, 'early')['early_commits'], 0)

    def test_deferrable_run_ahead_and_head_only_barrier(self):
        rows = m.scenario(kind='D', tasks=2, rounds=2)
        self.assertGreater(m.simulate(rows, 2, 'ahead')['logged'], 0)
        rows[1]['kind'] = 'H'
        self.assertEqual(m.simulate(rows, 2, 'ahead')['logged'], 0)
        self.assertEqual(m.simulate(rows, 2, 'ahead')['makespan_ns'],
                         m.simulate(rows, 2)['makespan_ns'])

    def test_filtered_poll_does_not_see_pending_send(self):
        rows = m.scenario(tasks=2, rounds=2)
        r = m.simulate(rows, 2, 'early')
        self.assertEqual(r['early_history'][0], (0, 1))
        self.assertGreater(r['publish'][0][1], rows[0]['ns'])

    def test_invalid_mode(self):
        with self.assertRaises(ValueError):
            m.simulate([], 0)


if __name__ == '__main__':
    unittest.main()
