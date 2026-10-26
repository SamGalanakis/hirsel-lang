#!/usr/bin/env python3
"""action_sizes_from_log.py sizes a compile from its 95th-percentile cores and
its peak memory, so one outlier does not reserve a core every action leaves idle."""
from pathlib import Path
import sys
import json
import tempfile
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'buck2'))
import action_sizes_from_log as sizing  # noqa: E402

GIB = 1024**3


def sample(cores, peak, target='hirsel-foo'):
    action = dict(profile='dev', target=target, phase='rlib', wall=2.0, cpu=1)
    row = dict(cpu_usec=str(int(cores * 2_000_000)), wall_ms='2000', anon_peak_bytes=str(peak),
               requested_kb='1048576', worker='w1')
    return action, row


class Sizes(unittest.TestCase):
    def test_refresh_retains_unobserved_targets_and_special_classes(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            commands, trace, usage, output = [root / name for name in ('commands', 'trace', 'usage', 'output')]
            commands.write_text(json.dumps(dict(display='hirsel-foo', category='rustc', cached=False,
                failed=False, end=101, wall=1, digest='abc:10', profile='dev', cpu=1, phase='rlib')))
            trace.write_text('\n'.join(map(json.dumps, [
                dict(event='dispatch', action_digest='abc-10', start_s=100, operation='op'),
                dict(event='usage', operation='op', sampled=True, outcome='COMPLETED', peak_memory_kb=200000),
            ])))
            usage.write_text('101\top=op\tcategory=rustc\texit=0\twall_ms=1000\tcpu_usec=1000000\t'
                             'peak_bytes=100000000\trequested_cpu=1\trequested_kb=262144\n')
            retained = dict(cpu_count=1, memory_kb=1048576)
            output.write_text(json.dumps(dict(profiles=dict(dev={'hirsel-foo': retained},
                release={'hirsel-bar': retained}), special_requests={'native_observation': retained})))
            with patch.object(sys, 'argv', ['sizes', '--commands', str(commands), '--pss', str(trace),
                                            '--usage', str(usage), '-o', str(output)]):
                sizing.main()
            result = json.loads(output.read_text())
            self.assertEqual(result['profiles']['release']['hirsel-bar'], retained)
            self.assertEqual(result['special_requests']['native_observation'], retained)
            self.assertEqual(result['profiles']['dev']['hirsel-foo']['memory_kb'], 262144)

    def test_cpu_uses_whole_run_and_memory_keeps_largest_peak(self):
        # 20 samples: 18 at one core, then 1.5 and an outlier of 3.5. The
        # nearest-rank p95 is the 19th value, 1.5, inside the slot share.
        matched = [sample(1.0, GIB) for _ in range(18)] + [sample(1.5, GIB), sample(3.5, 2 * GIB)]
        row = sizing.sizes(matched)['profiles']['dev']['hirsel-foo']
        self.assertEqual((row['p95_cores'], row['cpu_count']), (1.5, 1))
        # 2 GiB * 1.2, rounded to 64 MiB.
        self.assertEqual(row['memory_kb'], 2555904)
        # The outlier remains visible, but does not reserve idle CPUs.
        row = sizing.sizes(matched[1:])['profiles']['dev']['hirsel-foo']
        self.assertEqual((row['p95_cores'], row['cpu_count']), (3.5, 1))
        self.assertEqual([sizing.cpu_request(c) for c in (1.6, 1.61, 2.5, 9)], [1, 2, 2, 2])

    def test_small_pss_does_not_pay_the_compile_floor_or_anon_surcharge(self):
        action, row = sample(1, 40 * 1024**2)
        row['pss_peak_bytes'] = 80 * 1024**2
        result = sizing.sizes([(action, row)])['profiles']['dev']['hirsel-foo']
        self.assertEqual(result['memory_kb'], 131072)

    def test_digest_join_excludes_cache_hits_stale_and_ambiguous_executions(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            commands, trace = root / 'commands', root / 'trace'
            action = dict(display='third-party:small', category='rustc', cached=False, failed=False,
                          end=101, wall=1, digest='abc:10', profile='dev', cpu=1, phase='rlib')
            commands.write_text('\n'.join(json.dumps(a) for a in [action, action | {'cached': True},
                                  action | {'end': 80}]))
            dispatch = dict(event='dispatch', action_digest='abc-10', start_s=100, operation='op')
            peak = dict(event='usage', operation='op', sampled=True, outcome='COMPLETED', peak_memory_kb=200000)
            trace.write_text('\n'.join(map(json.dumps, [dispatch, peak])))
            records = {'op': {'wall_ms': '1000'}}
            joined = list(sizing.measured_commands([commands], [trace], records, 90, 110))
            self.assertEqual(len(joined), 1)
            self.assertEqual(joined[0][0]['target'], 'third-party:small')
            self.assertEqual(joined[0][1]['pss_peak_bytes'], 204800000)
            # Several quick memory retries are one operation, not ambiguous
            # concurrent actions. A killed attempt can peak above its retry.
            with trace.open('a') as out:
                out.write('\n' + json.dumps(dispatch | {'start_s': 98}))
                out.write('\n' + json.dumps(peak | {'outcome': 'KILLED_MEMORY', 'peak_memory_kb': 500000}))
            joined = list(sizing.measured_commands([commands], [trace], records, 90, 110))
            self.assertEqual(joined[0][1]['pss_peak_bytes'], 512000000)
            trace.write_text(trace.read_text() + '\n' + json.dumps(dispatch | {'operation': 'other'}))
            records['other'] = records['op']
            trace.write_text(trace.read_text() + '\n' + json.dumps(peak | {'operation': 'other'}))
            self.assertIsNone(list(sizing.measured_commands([commands], [trace], records, 90, 110))[0][1])


if __name__ == '__main__':
    unittest.main()
