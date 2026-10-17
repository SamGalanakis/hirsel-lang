#!/usr/bin/env python3
"""tools/buck2/pool_usage.py: reserved against used time per repository, lane and target."""
import json
from pathlib import Path
import sys
import tempfile
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
            '2\tcrate=-\ttest=-\top=c1\tcategory=-',  # a later row must not erase its name
            '2\tcrate=foreign\top=foreign\tcategory=rustc',
        ]
        rows = pool_usage.operations(usage, {'t1', 't2', 'c1'})
        self.assertNotIn('foreign', rows)
        text = pool_usage.report(pool_usage.executions(trace), pool_usage.names(rows), pool_usage.categories(rows), 'hirsel-lang', 1, 10)
        # In the hour: hirsel 4 + 1 + 1 CPU-h (two shards of one binary and a compile), lash 2; the 8-CPU run ended before it.
        self.assertIn('pool: 8.0 reserved CPU-hours', text)
        self.assertIn('hirsel-lang: 6.0 reserved CPU-hours, 75.0% of the pool\'s; used 2.0 (33%)', text)
        rows = {line.split()[0]: line.split() for line in text.splitlines()}
        self.assertEqual(rows['hirsel-x'][1:4], ['2', '5.00', '1.50'])
        self.assertEqual(rows['rustc'][2:5], ['1', '1.00', '0.50'])
        self.assertEqual(rows['gate'][1:3], ['2', '5.00'])

    def test_compile_and_link_report_execution_wall_and_cpu_without_queue_time(self):
        trace = [event('compile', 'hirsel-lang', 'gate', 2, 1024, 3, 5, 512, 103),
                 event('link', 'hirsel-lang', 'gate', 2, 1024, 2, 3, 512, 106)]
        usage = ['103\top=compile\tcategory=rustc\texit=0\twall_ms=3000\tcpu_usec=5000000\tpeak_bytes=512\trequested_cpu=2\trequested_kb=1024',
                 '106\top=link\tcategory=rustc\texit=0\twall_ms=2000\tcpu_usec=3000000\tpeak_bytes=512\trequested_cpu=2\trequested_kb=1024']
        rows = pool_usage.operations(usage, {'compile', 'link'})
        names, categories = pool_usage.names(rows), pool_usage.categories(rows)
        with tempfile.TemporaryDirectory() as temp:
            path = Path(temp) / 'events.jsonl'
            path.write_text('\n'.join([action('cached', 'rlib [pic]', 100, 3, cached=True),
                                       action('library', 'rlib [pic]', 100, 3),
                                       action('harness', 'link [pic]', 104, 2)]))
            pool_usage.attribute_rustc([path], rows, names, categories)
        self.assertEqual(categories, {'compile': 'rustc compile', 'link': 'rustc link'})
        self.assertEqual(names['compile'], 'rustc dev library rlib [pic]')
        self.assertEqual(names['link'], 'rustc dev harness link [pic]')
        text = pool_usage.report(pool_usage.executions(trace), names, categories, 'hirsel-lang', 1, 10)
        category_rows = {line.split()[1]: line.split()[2:] for line in text.splitlines()
                         if line.startswith('rustc ') and line.split()[1] in ('compile', 'link')}
        self.assertEqual(category_rows, {'compile': ['1', '3.00', '5.00'], 'link': ['1', '2.00', '3.00']})

    def test_ambiguous_worker_matches_keep_the_rustc_phase_unknown(self):
        usage = [f'103\top={op}\tcategory=rustc\texit=0\twall_ms=3000\tcpu_usec=5000000\tpeak_bytes=512\trequested_cpu=2\trequested_kb=1024'
                 for op in ('first', 'second')]
        rows = pool_usage.operations(usage, {'first', 'second'})
        names, categories = pool_usage.names(rows), pool_usage.categories(rows)
        with tempfile.TemporaryDirectory() as temp:
            path = Path(temp) / 'events.jsonl'
            path.write_text(action('library', 'rlib [pic]', 100, 3))
            pool_usage.attribute_rustc([path], rows, names, categories)
        self.assertEqual(set(categories.values()), {'rustc (phase unknown)'})


def action(target, phase, start, wall, cached=False):
    return json.dumps({'Event': {'data': {'SpanEnd': {'data': {'ActionExecution': {
        'key': {'owner': {'TargetLabel': {'label': {'package': 'root//', 'name': target},
                                        'configuration': {'full_name': 'root//tools/buck2:dev#hash'}}}},
        'name': {'category': 'rustc', 'identifier': phase}, 'failed': False,
        'commands': [{'details': {
            'command_kind': {'command': {'RemoteCommand': {
                'cache_hit': cached, 'queue_time_us': 999_000_000,
                'details': {'platform': {'properties': [{'name': 'cpu_count', 'value': '2'},
                                                       {'name': 'memory_kb', 'value': '1024'}]}}}}},
            'metadata': {'execution_time_us': wall * 1_000_000, 'start_time': [start, 0]},
        }}],
    }}}}}})


if __name__ == '__main__':
    unittest.main()
