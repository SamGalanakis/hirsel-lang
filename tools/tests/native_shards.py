#!/usr/bin/env python3
"""Native selections share the gate's cost TSVs and dynamically declared shards."""
import tempfile
from pathlib import Path
import sys
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'buck2'))
import native
import test_costs
import event_logs


class NativeShards(unittest.TestCase):
    def test_unknown_fifty_are_bounded_and_keep_both_backends(self):
        programs = {f'generated/success-{i}' for i in range(50)}
        self.assertEqual(len(native.targets(programs)), 100)
        parts = native.config(programs)[1].split('=', 1)[1].split(';')
        self.assertEqual(set(parts), programs)
        self.assertEqual(len(parts), len(programs))
        command = native.command(programs)
        self.assertEqual(command[command.index(native.NATIVE_TARGETS[0] + '-shard-1'):-3], native.targets(programs))

    def test_measured_costs_pack_longest_first_with_the_slower_backend(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            tables = root / 'tools/buck2/test-costs'
            tables.mkdir(parents=True)
            (tables / 'gate-native-changed.tsv').write_text('a\t400\nb\t200\nc\t200\nd\t100\nheavy\t850\n')
            (tables / 'gate-native-cranelift-changed.tsv').write_text('d\t300\n')
            parts = native.native_slices({'a', 'b', 'c', 'd', 'heavy', 'unknown'}, root)
            self.assertEqual(parts, [['heavy'], ['unknown'], ['a', 'c'], ['b', 'd']])
            self.assertEqual(sorted(p for part in parts for p in part), ['a', 'b', 'c', 'd', 'heavy', 'unknown'])
            self.assertEqual(parts, native.native_slices(reversed(['a', 'b', 'c', 'd', 'heavy', 'unknown']), root))

    def test_empty_and_single_selection_preserve_existing_target_names(self):
        self.assertEqual(native.targets(set()), [])
        self.assertEqual(native.targets({'x'}), native.NATIVE_TARGETS)

    def test_bad_measurements_fail_closed(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            table = root / 'tools/buck2/test-costs/gate-native-changed.tsv'
            table.parent.mkdir(parents=True)
            for bad in ['0', '-1', 'nan', 'inf']:
                table.write_text(f'a\t{bad}\n')
                with self.subTest(bad=bad), self.assertRaises(ValueError):
                    native.native_slices({'a'}, root)

    def test_native_costs_survive_the_gate_history_round_trip(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'history.tsv'
            event_logs.TsvTestHistory(path).append([dict(
                gate='example', target='gate-native-changed', shard=1,
                duration_seconds=12, date='2026-10-20', status=0,
                case_costs='{"a": 11.85}')])
            self.assertEqual(test_costs.history_costs(path, 'gate-native-changed', ''),
                             {'gate-native-changed': {'a': 11.85}})
        self.assertEqual(event_logs.case_costs('native case\ta\t11.850\t750000'), {'a': 11.85})
        self.assertEqual(event_logs.case_costs('native failed case\ta\t11.850\t750000'), {})

    def test_native_partial_measurements_preserve_unobserved_cases(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / 'gate-native-changed.tsv').write_text('old\t100.000\n')
            log = root / 'output.log'
            log.write_text('✓ Pass: root//:gate-native-changed (12.0s)\n'
                           'native case\tnew\t11.850\t750000\n')
            with patch.object(test_costs, 'TABLES', root), \
                 patch.object(sys, 'argv', ['test_costs.py', str(log)]):
                test_costs.main()
            self.assertEqual(test_costs.read_table(root / 'gate-native-changed.tsv'), {'old': 100, 'new': 11.85})

    def test_native_worker_measurements_are_accepted_by_the_cost_importer(self):
        walls, rows = test_costs.parse([
            '✓ Pass: root//:gate-native-changed-shard-1 (12.0s)',
            'native case conformance/SEM/a.hir 11.850 750000',
            '✓ Pass: root//:gate-native-cranelift-changed-shard-1 (9.0s)',
            'native case\tconformance/SEM/a.hir\t8.800\t600000'])
        self.assertEqual(rows, {'gate-native-changed': {'conformance/SEM/a.hir': 11.85},
                                'gate-native-cranelift-changed': {'conformance/SEM/a.hir': 8.8}})
        self.assertEqual(len(walls), 2)


if __name__ == '__main__':
    unittest.main()
