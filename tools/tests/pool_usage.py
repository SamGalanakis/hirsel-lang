#!/usr/bin/env python3
"""tools/buck2/pool_usage.py: reserved against used time per repository, lane and target."""
import json
from pathlib import Path
import sys
import unittest

sys.dont_write_bytecode = True
sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'buck2'))
import pool_usage  # noqa: E402


def event(op, repo, lane, cpu, memory_kb, wall_s, cpu_s, peak_kb, end_s, attempt='a'):
    return json.dumps({'event': 'usage', 'operation': op, 'attempt': attempt, 'identity': f'repo:{repo}|lane:{lane}|user:u',
                       'effective_cpu': cpu, 'effective_memory_kb': memory_kb, 'wall_time_ms': wall_s * 1000,
                       'cpu_time_ms': cpu_s * 1000, 'peak_memory_kb': peak_kb, 'end_s': end_s})


class Report(unittest.TestCase):
    def test_shares_targets_and_the_window(self):
        trace = [
            event('t1', 'hirsel-lang', 'gate', 4, 1 << 21, 3600, 3600, 1 << 20, 10_000),
            event('t1', 'hirsel-lang', 'gate', 4, 1 << 21, 3600, 3600, 1 << 20, 10_000),  # a resent event
            event('t2', 'hirsel-lang', 'gate', 2, 1 << 20, 1800, 1800, 1 << 19, 10_000),
            event('c1', 'hirsel-lang', 'focus', 1, 1 << 20, 3600, 1800, 1 << 18, 9_000),
            event('l1', 'lash', 'ci', 2, 1 << 20, 3600, 3600, 1 << 20, 10_000),
            event('old', 'lash', 'ci', 8, 1 << 20, 3600, 3600, 1 << 20, 10_000 - 7200),
            '{"event":"dispatch","operation":"t1"}',
        ]
        usage = [
            '1\tcrate=-\ttest=run:1:root//:hirsel-x-shard-1\top=t1\tcategory=-',
            '1\tcrate=-\ttest=run:2:root//:hirsel-x-shard-2\top=t2\tcategory=-',
            '1\tcrate=hirsel_types\ttest=-\top=c1\tcategory=rustc',
        ]
        text = pool_usage.report(pool_usage.executions(trace), pool_usage.names(usage), 'hirsel-lang', 1, 10)
        # In the hour: hirsel 4 + 1 + 1 CPU-h (two shards of one binary and a compile), lash 2; the 8-CPU run ended before it.
        self.assertIn('pool: 8.0 reserved CPU-hours', text)
        self.assertIn('hirsel-lang: 6.0 reserved CPU-hours, 75.0% of the pool\'s; used 2.0 (33%)', text)
        rows = {line.split()[0]: line.split() for line in text.splitlines()}
        self.assertEqual(rows['hirsel-x'][1:4], ['2', '5.00', '1.50'])
        self.assertEqual(rows['rustc'][2:5], ['1', '1.00', '0.50'])
        self.assertEqual(rows['gate'][1:3], ['2', '5.00'])


if __name__ == '__main__':
    unittest.main()
