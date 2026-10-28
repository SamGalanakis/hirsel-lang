#!/usr/bin/env python3
"""Native TSan qualification requires four distinct full-width P witnesses."""
import contextlib
import io
import os
from pathlib import Path
import runpy
import subprocess
import sys
import unittest
from unittest.mock import patch

SCRIPT = Path(__file__).resolve().parents[1] / 'workers-tsan'


class NativeQualification(unittest.TestCase):
    def run_gate(self, width, stderr, status=0):
        result = subprocess.CompletedProcess([], status, '4 passed\n', stderr)
        output = io.StringIO()
        with patch.dict(os.environ, {'HIRSEL_NATIVE_CPU_HARNESS': 'unused-harness'}), \
                patch.object(sys, 'argv', [str(SCRIPT), '--native', str(width)]), \
                patch.object(subprocess, 'run', return_value=result), \
                contextlib.redirect_stdout(output), contextlib.redirect_stderr(output):
            with self.assertRaises(SystemExit) as exit:
                runpy.run_path(str(SCRIPT), run_name='__main__')
        return exit.exception.code

    def witnesses(self, width):
        return [f'CPU {policy} {backend} P({width}): switch-free overlap, ownership=Ok(...)\n'
                for policy in ['tasks.free@1', 'tasks.cooperative@1']
                for backend in ['llvm', 'cranelift']]

    def test_value_passes_without_exclusive_cpu_ownership_are_not_qualification(self):
        for width in [4, 8]:
            with self.subTest(width=width):
                refusal = (f'CPU tasks.free@1 llvm W{width}: all workers progressed; '
                           f'P({width}) NOT QUALIFIED, ownership=Err("no visible private cgroup")\n')
                self.assertNotEqual(self.run_gate(width, refusal), 0)

    def test_every_policy_and_emitter_must_have_its_own_full_width_witness(self):
        for width in [4, 8]:
            with self.subTest(width=width):
                rows = self.witnesses(width)
                self.assertEqual(self.run_gate(width, ''.join(rows)), 0)
                self.assertNotEqual(self.run_gate(width, rows[0] * 4), 0)
                self.assertNotEqual(self.run_gate(width, ''.join(rows[:-1])), 0)

    def test_sanitizer_failure_status_is_preserved_even_with_witnesses(self):
        self.assertEqual(self.run_gate(8, ''.join(self.witnesses(8)), 66), 66)


if __name__ == '__main__':
    unittest.main()
