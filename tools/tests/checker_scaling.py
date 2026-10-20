#!/usr/bin/env python3
"""Measurement verdicts must distinguish tolerated N log N from cubic work."""
import importlib.util
import math
from pathlib import Path
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location('checker_scaling', Path(__file__).resolve().parents[1] / 'checker-scaling.py')
scaling = importlib.util.module_from_spec(spec)
spec.loader.exec_module(scaling)
BASELINE = {'status': 0, 'instructions': 1000, 'rss_kb': 1000}


def samples(work, status=0):
    return {n: {'status': status, 'instructions': 1000 + work(n), 'rss_kb': 1000 + work(n), 'detail': ''}
            for n in scaling.SIZES}


class Growth(unittest.TestCase):
    def test_linearithmic_work_passes_and_cubic_work_fails(self):
        self.assertEqual(scaling.judge(samples(lambda n: 100 * n * math.log2(n)), BASELINE), [])
        failures = scaling.judge(samples(lambda n: n**3), BASELINE)
        self.assertTrue(any('instructions' in error for error in failures), failures)
        self.assertTrue(any('rss_kb' in error for error in failures), failures)

    @patch.dict(scaling.EXPECTED, {'match_try_fields': 'owner'})
    def test_expected_failures_must_fail_by_growth_and_unexpected_passes_fail(self):
        family = 'match_try_fields'
        self.assertEqual(scaling.verdict(family, samples(lambda n: n**3), BASELINE)[0], True)
        self.assertEqual(scaling.verdict(family, samples(lambda n: n), BASELINE)[0], False)
        broken = samples(lambda n: n**3)
        broken[10] = {'status': 1, 'instructions': None, 'rss_kb': 1, 'detail': 'type error'}
        self.assertEqual(scaling.verdict(family, broken, BASELINE)[0], False)
        timed_out = samples(lambda n: n)
        timed_out[160] = {'status': 124, 'instructions': None, 'rss_kb': 1, 'detail': 'exceeded'}
        self.assertEqual(scaling.verdict(family, timed_out, BASELINE), (True, scaling.verdict(family, timed_out, BASELINE)[1]))
        self.assertEqual(scaling.verdict('if_chain', timed_out, BASELINE)[0], False)

    def test_all_families_have_all_required_sizes(self):
        for family in scaling.FAMILIES:
            programs = [scaling.program(family, n) for n in scaling.SIZES]
            self.assertEqual(len(set(programs)), 3, family)
            self.assertTrue(all('fn probe' in program for program in programs))


if __name__ == '__main__':
    unittest.main()
