#!/usr/bin/env python3
"""action_sizes_from_log.py sizes a compile from its 95th-percentile cores and
its peak memory, so one outlier does not reserve a core every action leaves idle."""
from pathlib import Path
import sys
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'buck2'))
import action_sizes_from_log as sizing  # noqa: E402

GIB = 1024**3


def sample(cores, peak, target='hirsel-foo'):
    action = dict(profile='dev', target=target, phase='rlib', wall=2.0, cpu=1)
    row = dict(cpu_usec=str(int(cores * 2_000_000)), wall_ms='2000', anon_peak_bytes=str(peak),
               requested_kb='1048576', worker='w1')
    return action, row


class Sizes(unittest.TestCase):
    def test_cpus_follow_the_nearest_rank_p95_and_memory_the_largest_peak(self):
        # 20 samples: 18 at one core, then 1.5 and an outlier of 3.5. The
        # nearest-rank p95 is the 19th value, 1.5, inside the slot share.
        matched = [sample(1.0, GIB) for _ in range(18)] + [sample(1.5, GIB), sample(3.5, 2 * GIB)]
        row = sizing.sizes(matched)['profiles']['dev']['hirsel-foo']
        self.assertEqual((row['p95_cores'], row['cpu_count']), (1.5, 1))
        # 2 GiB * 1.25 + 512 MiB = 3 GiB.
        self.assertEqual(row['memory_kb'], 3 * 1024 * 1024)
        # Under 20 samples the maximum decides: 3.5 cores is four CPUs.
        row = sizing.sizes(matched[1:])['profiles']['dev']['hirsel-foo']
        self.assertEqual((row['p95_cores'], row['cpu_count']), (3.5, 4))
        self.assertEqual([sizing.cpu_request(c) for c in (1.6, 1.61, 2.5, 9)], [1, 2, 3, 4])


if __name__ == '__main__':
    unittest.main()
