#!/usr/bin/env python3
"""The generated repair reference retains rule-specific diagnostic guidance."""
from pathlib import Path
import runpy
import unittest

ROOT = Path(__file__).resolve().parents[2]
GENERATOR = runpy.run_path(str(ROOT / 'tools/generate-diagnostics.py'))


class GenerateDiagnostics(unittest.TestCase):
    def test_lang4_includes_near_name_guidance(self):
        page = GENERATOR['render']()
        names = page.split('\n## LANG-4: Names\n', 1)[1].split('\n## ', 1)[0]
        for guidance in [
            'up to three closest accessible names',
            'Equally close names are\nshown in lexical order',
            'When there is exactly one suggestion and an exact identifier span is available,\n'
            'JSON `fix` contains a machine-applicable replacement of that identifier.',
            'Suggestions respect package exposure and\nvisibility',
            '[rustc_span::edit_distance](https://github.com/rust-lang/rust/',
        ]:
            with self.subTest(guidance=guidance):
                self.assertIn(guidance, names)


if __name__ == '__main__':
    unittest.main()
