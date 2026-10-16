"""A contended pilot must not under-size later uncontended samples."""
import importlib.machinery
import importlib.util
import random
import sys
import unittest
from pathlib import Path
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
loader = importlib.machinery.SourceFileLoader('native_collector', str(ROOT / 'run'))
spec = importlib.util.spec_from_loader(loader.name, loader)
collector = importlib.util.module_from_spec(spec)
loader.exec_module(collector)


class CalibrationTests(unittest.TestCase):
    def collect(self, pilot_wall):
        calls = []

        def sample(participant, case, repeats, *args, **kwargs):
            # The fastest reference performs 10ms CPU work/epoch, but the host
            # stalls pilots. The subsequent complete cell runs without stalls.
            cpu = repeats * (0.01 if participant == 'c/system' else 0.02)
            calls.append((participant, repeats))
            return dict(participant=participant, repeats=repeats, oracle_ok=True,
                        seconds=max(cpu, pilot_wall) if len(calls) <= 8 else cpu,
                        costs=dict(user_seconds=round(cpu, 2), system_seconds=0.))

        with patch.object(collector, 'sample', sample), patch('builtins.print'):
            return collector.collect_case({'id': 'contended'},
                ['c/system', 'regions-on-whole'], 31, None, None, random.Random(1))

    def test_host_stalls_cannot_create_short_uncontended_samples(self):
        record = self.collect(2.)
        samples = [s for r in record['warmups'] + record['rounds'] for s in r['samples']]
        self.assertGreaterEqual(min(s['seconds'] for s in samples), 0.250)
        self.assertEqual(len(record['warmups']), 5)
        self.assertEqual(len(record['rounds']), 30)
        self.assertTrue(all(s['oracle_ok'] for s in samples))
        self.assertEqual({s['repeats'] for s in samples}, {record['repeats']})

    def test_repeat_count_depends_on_work_not_scheduler_delay(self):
        self.assertEqual(self.collect(2.)['repeats'], self.collect(0.)['repeats'])

    def test_calibration_cannot_lower_headroom(self):
        with self.assertRaises(ValueError):
            collector.collect_case({}, [], 31, None, None, None, calibration_seconds=.25)
