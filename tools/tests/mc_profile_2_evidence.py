"""Keep M0b's checked-in cost-model tables tied to their inputs and policy."""
import json
from pathlib import Path
import subprocess
import sys
import unittest

ROOT = Path(__file__).resolve().parents[2]
HERE = ROOT/'reports/mc-profile-2-2026-10-20'


class Evidence(unittest.TestCase):
    def test_tables_reproduce_without_external_tools(self):
        subprocess.run([sys.executable, str(HERE/'analyze.py'), '--check'], check=True)

    def test_complete_interleaved_cost_cells_and_instruction_counts(self):
        data = json.loads((HERE/'costs.json').read_text())
        self.assertEqual(len(data['samples']), 80)
        cells = {(s['round'], s['width'], s['mode']) for s in data['samples']}
        self.assertEqual(cells, {(r, w, m) for r in range(10) for w in (4, 8) for m in range(4)})
        for sample in data['samples']:
            self.assertGreater(sample['instructions'], 0)
            self.assertGreater(sample['cycles'], 0)
            self.assertEqual(sample['checksum'], sample['width']*sample['iterations'])
            self.assertEqual(len(sample['smt_busy']), sample['width'])


if __name__ == '__main__':
    unittest.main()
