"""Native reports retain physical payload classes and distinct RC operations."""
import sys
import unittest
from pathlib import Path
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from suite import parse_measurement

REPORT = '''hirsel-measurement 1
class 8 2 2
class large 1 1
count_increments 3
count_decrements 5
cow_detaches 0
deep_copies 0
copied_blocks 0
copied_bytes 0
reuse_hits 1
reuse_misses 2
live_bytes 0
peak_live_bytes 72
region_high_water_bytes 0
region_pool_high_water_bytes 0
arena_high_water_bytes unavailable
'''

class NativeMeasurement(unittest.TestCase):
    def test_preserves_units_and_unavailable_arena(self):
        row = parse_measurement(REPORT)
        self.assertEqual(row['classes']['8'], {'allocations': 2, 'frees': 2})
        self.assertEqual(row['count_increments'], 3)
        self.assertEqual(row['count_decrements'], 5)
        self.assertEqual(row['arena_high_water_bytes'], 'unavailable')
        self.assertNotIn('arena_peak_reserved_bytes', row)

    def test_incomplete_duplicate_negative_or_wrong_version_refuses(self):
        for report in (REPORT.replace('hirsel-measurement 1', 'hirsel-measurement 2'),
                       REPORT.replace('copied_bytes 0\n', ''),
                       REPORT + 'live_bytes 0\n', REPORT + 'class 8 1 1\n',
                       REPORT.replace('reuse_hits 1', 'reuse_hits -1'),
                       REPORT.replace('class large 1 1', 'class large -1 1'),
                       REPORT.replace('arena_high_water_bytes unavailable', 'arena_high_water_bytes 0')):
            with self.subTest(report=report), self.assertRaises(ValueError):
                parse_measurement(report)
