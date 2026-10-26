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
    def test_full_refresh_without_native_runs_preserves_native_evidence(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            retained = dict(cpu_count=1, memory_kb=4194304, cpu_class='')
            (root / 'test-sizes.json').write_text(json.dumps({'gate-native-full': retained}))
            usage = root / 'usage.log'
            usage.write_text('10\ttest=run:0:root//:hirsel-conformance-test-it\top=abc\twall_ms=1000\t'
                             'cpu_usec=1200000\trequested_cpu=1\trequested_kb=524288\n')
            trace = root / 'trace.jsonl'
            trace.write_text(json.dumps(dict(event='usage', operation='abc', sampled=True,
                                             peak_memory_kb=2400000)))
            with patch.object(sizes, 'HERE', root), patch.object(sys, 'argv', [
                    'sizes', '--usage', str(usage), '--pss', str(trace)]):
                sizes.main()
            rows = json.loads((root / 'test-sizes.json').read_text())
            self.assertEqual(rows['gate-native-full'], retained)
            self.assertGreaterEqual(rows['hirsel-conformance-test-it']['memory_kb'], 2400000 * 1.2)

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
            self.assertEqual(rows['hirsel-cli-test-cli']['memory_kb'], 2424832)
            self.assertIn('"gate-agree-native-full": [1, 4194304, ""]', sizes.requests_bzl(rows))
            before = (root / 'test-sizes.json').read_bytes()
            with patch.object(sizes, 'HERE', root), patch.object(sys, 'argv', [
                    'sizes', '--usage', str(usage), '--pss', str(trace), '--only', 'missing']):
                with self.assertRaises(SystemExit):
                    sizes.main()
            self.assertEqual((root / 'test-sizes.json').read_bytes(), before)

    def test_completed_retry_uses_peak_instead_of_geometric_retry_overshoot(self):
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
        self.assertEqual(full['memory_kills'], 0)
        self.assertEqual(full['memory_kb'], 5111808)
        self.assertEqual(full['cpu_count'], 1)
        for name in sizes.NATIVE_STAGES - {'gate-native-full'}:
            self.assertEqual(rows[name]['fallback_from'], 'gate-native-full')
            self.assertEqual(rows[name]['memory_kb'], full['memory_kb'])
            self.assertEqual(rows[name]['runs'], 0)

    def test_pss_margin_rounds_up_and_colored_kills_remain_a_floor(self):
        sample = dict(cpu=1, wall=1, requested_cpu=1, requested_kb=2097152, end=1)
        self.assertEqual(sizes.size('gate-native-full', [sample], [4210843], [])['memory_kb'], 5111808)
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'kills.txt'
            path.write_text('Uuid(abc), \x1b[33mreserved_kb\x1b[0m: 2097152')
            floors = sizes.kills(path, {'abc': 'gate-native-full'})
        self.assertEqual(sizes.size('gate-native-full', [sample], [4210843], floors['gate-native-full'])['memory_kb'], 5111808)
        with self.assertRaisesRegex(ValueError, 'no measured native stage'):
            sizes.native_rows({}, defaultdict(list), defaultdict(list))

    def test_virtual_memory_guard_is_independent_of_resident_reservation(self):
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
        # 1.75x the stopped run's reservation, rounded up to 64 MiB.
        self.assertEqual(rows['hirsel-cli-test-cli']['memory_kb'], 1048576)
        self.assertEqual(rows['hirsel-cli-test-cli']['guard_limit_kb'], 3670016)
        self.assertEqual(rows['hirsel-driver-test-it-precheck'],
                         dict(cpu_count=1, memory_kb=131072, guard_limit_kb=10092544, cpu_class='', runs=0, pss_runs=0, memory_kills=0))
        self.assertEqual(rows['hirsel-lir-test-it']['memory_kb'], 131072)
        # A measured need above 1.75x sets the floor: half the limit it needed.
        self.assertEqual(rows['hirsel-intern-unittest']['memory_kb'], 131072)
        self.assertEqual(rows['hirsel-intern-unittest']['guard_limit_kb'], 4194304)

    def test_sustained_cpu_and_small_pss_replace_peak_cpu_and_large_floor(self):
        samples = [dict(cpu=1, wall=1, requested_cpu=4, requested_kb=524288, end=i) for i in range(20)]
        samples.append(dict(cpu=4, wall=1, requested_cpu=4, requested_kb=524288, end=21))
        row = sizes.size('gate-agree-full', samples, [100000], [])
        self.assertEqual(row['cpu_count'], 1)
        self.assertEqual(row['memory_kb'], 131072)
        parallel = [dict(cpu=3.5, wall=1, requested_cpu=4, requested_kb=524288, end=1)]
        self.assertEqual(sizes.size('gate-agree-full', parallel, [100000], [])['cpu_count'], 2)

    def test_memory_uses_samples_at_the_new_cpu_count_when_available(self):
        with tempfile.TemporaryDirectory() as directory:
            trace = Path(directory) / 'trace.jsonl'
            trace.write_text('\n'.join(json.dumps(e) for e in [
                dict(event='usage', operation='one', sampled=True, effective_cpu=1, peak_memory_kb=100000),
                dict(event='usage', operation='four', sampled=True, effective_cpu=4, peak_memory_kb=1000000),
            ]))
            operations = {'one': 'binary', 'four': 'binary'}
            self.assertEqual(sizes.pss_peaks([trace], operations, {'binary': 1})['binary'], [100000])
            # With no matching CPU evidence, keep the largest available peak.
            self.assertEqual(sizes.pss_peaks([trace], {'four': 'binary'}, {'binary': 1})['binary'], [1000000])

    def test_an_unfinished_memory_kill_keeps_a_safe_floor(self):
        with tempfile.TemporaryDirectory() as directory:
            trace = Path(directory) / 'trace.jsonl'
            trace.write_text(json.dumps(dict(event='usage', operation='abc', outcome='KILLED_MEMORY',
                                             effective_memory_kb=1048576)))
            self.assertEqual(sizes.escalation_floors([trace], {'abc': 'binary'})['binary'], [2516583])


if __name__ == '__main__':
    unittest.main()
