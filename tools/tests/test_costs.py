#!/usr/bin/env python3
"""Cost logs retain exact inventories and history reports count distinct gates."""
import contextlib
import importlib.util
import io
import json
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'buck2'))
SPEC = importlib.util.spec_from_file_location('measured_costs', Path(__file__).resolve().parents[1] / 'buck2/test_costs.py')
costs = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(costs)


class Costs(unittest.TestCase):
    def test_history_costs_merge_latest_cases_without_erasing_unobserved_cases(self):
        with tempfile.TemporaryDirectory() as scratch:
            path = Path(scratch) / 'results.tsv'
            binary = costs.SHARDED[0]
            def row(date, **cases):
                return dict(gate=date, target=binary, shard=1, duration_seconds=1,
                            date=date, status=1, case_costs=json.dumps(cases))
            # Appends from concurrent gates need not be in timestamp order.
            costs.event_logs.TsvTestHistory(path).append([
                row('2026-10-19', a=2.023), row('2026-10-18', a=1, b=3),
                dict(row('2026-10-19'), case_costs='')])
            self.assertEqual(costs.history_costs(path, binary, ''), {binary: {'a': 2.023, 'b': 3}})
            self.assertEqual(costs.history_costs(path, 'all', '2026-10-19'), {binary: {'a': 2.023}})
            table = Path(scratch) / f'{binary}.tsv'
            table.write_text('unobserved\t4.0\na\t0.0\n')
            with patch.object(costs, 'TABLES', Path(scratch)), \
                 patch.object(sys, 'argv', ['test_costs.py', '--from-history', str(path), '--binary', binary]), \
                 contextlib.redirect_stdout(io.StringIO()):
                costs.main()
            self.assertEqual(costs.read_table(table), {'unobserved': 4, 'a': 2.023, 'b': 3})

    def test_inventory_keeps_macro_and_ignored_names_and_stops_at_other_targets(self):
        binary = costs.SHARDED[0]
        lines = [f'[date] ✓ Pass: root//:{binary}-shard-1 (1:02.3s)',
                 '---- STDOUT ----', 'generated::case_0: test', 'ignored::slow: test',
                 '✓ Pass: root//:other (0.1s)', 'unrelated: test']
        self.assertEqual(costs.inventory(lines), {binary: {'generated::case_0', 'ignored::slow'}})
        lines = [f'✓ Pass: root//:{binary}-shard-1 (1.0s)',
                 'test native::case ... ok <0.5s>', 'test expected_panic - should panic ... ok <0.2s>']
        walls, rows = costs.parse(lines)
        self.assertEqual(walls[f'{binary}-shard-1'], ('Pass', 1.0))
        self.assertEqual(rows[binary], {'native::case': 0.5, 'expected_panic': 0.2})

    def test_complete_measurement_uses_generated_graph_before_resizing(self):
        with tempfile.TemporaryDirectory() as scratch:
            root = Path(scratch)
            here = root / 'tools/buck2'
            here.mkdir(parents=True)
            (root / 'BUCK').write_text('hirsel_test(\n    name = "binary",\n    shards = 5,\n)\n'
                                       'hirsel_test(\n    name = "single",\n)\n')
            with patch.object(costs, 'HERE', here):
                self.assertEqual(costs.configured_shards('binary'), 5)
                self.assertEqual(costs.configured_shards('single'), 1)
                with self.assertRaisesRegex(ValueError, 'no generated test target'):
                    costs.configured_shards('missing')

    def test_history_reports_medians_maxima_and_refuses_insufficient_gates(self):
        with tempfile.TemporaryDirectory() as scratch:
            path = Path(scratch) / 'results.tsv'
            sink = costs.event_logs.TsvTestHistory(path)
            rows = [dict(gate=str(i), target='driver', shard=1, duration_seconds=seconds,
                         date='2026-10-19T20:00:00+00:00', status=1)
                    for i, seconds in enumerate([1, 2, 3, 4, 100])]
            sink.append(rows + [rows[-1]])
            sink.append([dict(rows[0], gate='old', date='2026-10-18', duration_seconds=1000)])
            output = io.StringIO()
            with contextlib.redirect_stdout(output):
                costs.history_report(path, 'driver', '2026-10-19', 5)
            self.assertIn('1\t5\t3.0\t100.0', output.getvalue())
            with contextlib.redirect_stdout(io.StringIO()), self.assertRaisesRegex(SystemExit, 'at least 6'):
                costs.history_report(path, 'driver', '2026-10-19', 6)
            with self.assertRaisesRegex(SystemExit, 'no timing history'):
                costs.history_report(path, 'missing', '', 5)


if __name__ == '__main__':
    unittest.main()
