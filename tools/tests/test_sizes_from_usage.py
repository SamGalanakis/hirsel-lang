#!/usr/bin/env python3
"""Native requests retain PSS, escalation and missing-stage evidence."""
from collections import defaultdict
import json
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.dont_write_bytecode = True
sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'buck2'))
import test_sizes_from_usage as sizes  # noqa: E402


class NativeSizes(unittest.TestCase):
    def test_partial_refresh_preserves_unobserved_native_requests(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            retained = {'cpu_count': 1, 'memory_kb': 4194304, 'cpu_class': '', 'fallback_from': 'gate-native-full'}
            (root / 'test-sizes.json').write_text(json.dumps({'gate-agree-native-full': retained}))
            usage = root / 'usage.log'
            usage.write_text('10\ttest=run:-:root//:hirsel-cli-test-cli\top=abc\twall_ms=1000\t'
                             'cpu_usec=1200000\trequested_cpu=1\trequested_kb=524288\n')
            trace = root / 'trace.jsonl'
            trace.write_text(json.dumps(dict(event='usage', operation='abc', sampled=True, peak_memory_kb=2000000)))
            with patch.object(sizes, 'HERE', root), patch.object(sys, 'argv', [
                    'sizes', '--usage', str(usage), '--pss', str(trace), '--only', 'hirsel-cli-test-cli']):
                sizes.main()
            rows = json.loads((root / 'test-sizes.json').read_text())
            self.assertEqual(rows['gate-agree-native-full'], retained)
            self.assertEqual(rows['hirsel-cli-test-cli']['memory_kb'], 1310720)
            self.assertIn('"gate-agree-native-full": [1, 4194304, ""]', (root / 'test_sizes.bzl').read_text())
            before = (root / 'test-sizes.json').read_bytes()
            with patch.object(sizes, 'HERE', root), patch.object(sys, 'argv', [
                    'sizes', '--usage', str(usage), '--pss', str(trace), '--only', 'missing']):
                with self.assertRaises(SystemExit):
                    sizes.main()
            self.assertEqual((root / 'test-sizes.json').read_bytes(), before)

    def test_pss_and_actual_retry_floor_are_shared_by_shards(self):
        with tempfile.TemporaryDirectory() as directory:
            usage = Path(directory) / 'actions.log'
            usage.write_text('10\ttest=run:1:root//:gate-native-full-shard-1\top=abc\tevent=start\n'
                             '20\ttest=run:2:root//:gate-native-full-shard-2\top=abc\twall_ms=1000\t'
                             'cpu_usec=1200000\trequested_cpu=1\trequested_kb=4194304\n')
            trace = Path(directory) / 'trace.jsonl'
            trace.write_text('\n'.join(json.dumps(e) for e in [
                dict(event='usage', operation='abc', outcome='KILLED_MEMORY', sampled=True,
                     peak_memory_kb=4210843, effective_memory_kb=2097152),
                dict(event='usage', operation='abc', outcome='COMPLETED', sampled=True,
                     peak_memory_kb=2000000, effective_memory_kb=4194304),
            ]))
            found, operations = sizes.runs([usage])
            peaks = sizes.pss_peaks([trace], operations)
            floors = sizes.escalation_floors([trace], operations)
            rows = sizes.native_rows(found, peaks, floors)
        self.assertEqual(set(rows), sizes.NATIVE_STAGES)
        full = rows['gate-native-full']
        self.assertEqual(full['pss_peak_mib'], 4112)
        self.assertEqual(full['memory_kills'], 1)
        self.assertEqual(full['memory_kb'], 4194304)
        self.assertEqual(full['cpu_count'], 1)
        for name in sizes.NATIVE_STAGES - {'gate-native-full'}:
            self.assertEqual(rows[name]['fallback_from'], 'gate-native-full')
            self.assertEqual(rows[name]['memory_kb'], full['memory_kb'])
            self.assertEqual(rows[name]['runs'], 0)

    def test_pss_margin_rounds_up_and_colored_kills_remain_a_floor(self):
        sample = dict(cpu=1, wall=1, requested_cpu=1, requested_kb=2097152, end=1)
        self.assertEqual(sizes.size('gate-native-full', [sample], [4210843], [])['memory_kb'], 2621440)
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'kills.txt'
            path.write_text('Uuid(abc), \x1b[33mreserved_kb\x1b[0m: 2097152')
            floors = sizes.kills(path, {'abc': 'gate-native-full'})
        self.assertEqual(sizes.size('gate-native-full', [sample], [4210843], floors['gate-native-full'])['memory_kb'], 3670016)
        with self.assertRaisesRegex(ValueError, 'no measured native stage'):
            sizes.native_rows({}, defaultdict(list), defaultdict(list))

    def test_a_guard_stop_is_a_kill_floor_and_sizes_an_unmeasured_binary(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / 'test-sizes.json').write_text(json.dumps(
                {'hirsel-cli-test-cli': {'cpu_count': 2, 'memory_kb': 1048576, 'cpu_class': ''}}))
            (root / 'guard-stops.json').write_text(json.dumps({
                'hirsel-cli-test-cli': [{'reserved_kb': 1048576}],
                'hirsel-intern-unittest': [{'reserved_kb': 524288, 'needed_kb': 4194304}],
                'hirsel-driver-test-it-precheck': [{'reserved_kb': 2883584, 'cpu_count': 2}],
            }))
            usage = root / 'usage.log'
            usage.write_text('10\ttest=run:-:root//:hirsel-lir-test-it\top=abc\twall_ms=1000\t'
                             'cpu_usec=1200000\trequested_cpu=1\trequested_kb=524288\n')
            trace = root / 'trace.jsonl'
            trace.write_text(json.dumps(dict(event='usage', operation='abc', sampled=True, peak_memory_kb=100000)))
            with patch.object(sizes, 'HERE', root), patch.object(sys, 'argv', [
                    'sizes', '--usage', str(usage), '--pss', str(trace), '--only', 'hirsel-lir-test-it']):
                sizes.main()
            rows = json.loads((root / 'test-sizes.json').read_text())
        # 1.75x the stopped run's reservation, rounded up to 256 MiB.
        self.assertEqual(rows['hirsel-cli-test-cli']['memory_kb'], 1835008)
        self.assertEqual(rows['hirsel-driver-test-it-precheck'],
                         dict(cpu_count=2, memory_kb=5242880, cpu_class='', runs=0, pss_runs=0, memory_kills=1))
        self.assertEqual(rows['hirsel-lir-test-it']['memory_kb'], 524288)
        # A measured need above 1.75x sets the floor: half the limit it needed.
        self.assertEqual(rows['hirsel-intern-unittest']['memory_kb'], 2097152)


if __name__ == '__main__':
    unittest.main()
