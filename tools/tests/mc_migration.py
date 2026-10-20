"""Migration-rate placement controls with hand-computable worker assignments."""
import importlib.util
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location('migration', Path(__file__).resolve().parents[1]/'mc-migration.py')
m = importlib.util.module_from_spec(spec)
spec.loader.exec_module(m)


def row(task, ns, after, operation='task_canceled'):
    return dict(task=task, ns=ns, after=after, operation=operation)


class Placement(unittest.TestCase):
    def test_serial_task_stays_on_its_worker(self):
        rows = [row([], 10, i) for i in range(4)]
        result = m.simulate(rows, 8, 1)
        self.assertEqual(result['migrations'], 0)
        self.assertEqual(result['switches'], 4)
        self.assertEqual(result['worker_task_changes'], 1)

    def test_busy_previous_worker_causes_a_steal(self):
        rows = [row([], 0, 0), row([0], 10, 1), row([1], 100, 1),
                row([], 10, 1), row([0], 1000, 2), row([], 1, 4)]
        result = m.simulate(rows, 2, 1)
        self.assertEqual(result['migrations'], 1)
        self.assertEqual(result['revisits'], 3)

    def test_known_keep_ops_form_one_turn(self):
        rows = [row([], 3, 0, 'mutex_take'), row([], 7, 1, 'mutex_put'),
                row([0], 8, 2)]
        self.assertEqual(m.turns(rows), [row([], 10, 0, 'mutex_put'), row([0], 8, 1)])

    def test_keep_never_joins_different_tasks(self):
        rows = [row([], 3, 0, 'mutex_take'), row([0], 7, 1)]
        self.assertEqual(len(m.turns(rows)), 2)


if __name__ == '__main__':
    unittest.main()
