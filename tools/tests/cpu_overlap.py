#!/usr/bin/env python3
"""The private CPU runner preserves exact grants, failures and owned cleanup."""
import contextlib
import importlib.machinery
import io
import json
import os
import pwd
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

SCRIPT = Path(__file__).resolve().parents[1] / 'cpu-overlap'
runner = importlib.machinery.SourceFileLoader('cpu_overlap', str(SCRIPT)).load_module()


class PrivateScopes(unittest.TestCase):
    def invoke(self, status=0, timeout=False, stdout="test result: ok. 1 passed; 0 failed; 0 ignored;\n"):
        with tempfile.TemporaryDirectory() as scratch:
            directory = Path(scratch)
            for kind, widths in [('ordinary', [4, 8, 16]), ('tsan', [4, 8])]:
                for width in widths:
                    (directory / f'{kind}-{width}.json').write_text(json.dumps({
                        'root': scratch, 'command': ['launcher', 'harness'],
                        'env': {'HIRSEL_MEMORY_LIMIT': '2097152K', 'HIRSEL_RUNTIME_ARCHIVES': ['runtime-archive']}}))
            result = subprocess.CompletedProcess([], status, stdout, "")
            account = pwd.struct_passwd(('audit', 'x', os.getuid(), os.getgid(), '', scratch, '/bin/sh'))
            side_effect = subprocess.TimeoutExpired([], 600) if timeout else None
            with patch.object(runner.pwd, 'getpwuid', return_value=account), \
                    patch.object(runner.subprocess, 'run', return_value=result, side_effect=side_effect) as run, \
                    contextlib.redirect_stdout(io.StringIO()):
                if timeout:
                    run.side_effect = [subprocess.TimeoutExpired([], 600), result]
                failed = runner.run(directory, list(range(16)), None, os.getuid(), ['ordered-4'])
            return failed, run.call_args_list

    def test_normal_executables_survive_tsan_materialization_at_the_same_paths(self):
        with tempfile.TemporaryDirectory() as scratch:
            root = Path(scratch)
            prepared = root / 'commands'
            prepared.mkdir()
            for name in ['harness', 'compiler']:
                (root / name).write_text('ordinary')
            (root / 'products').mkdir()
            (root / 'products/index').write_text('ordinary')
            data = {'command': ['launcher', 'harness'], 'env': {
                'HIRSEL_TEST_PRODUCTS': ['products'], 'HIRSEL_PRODUCTS': ['products'],
                'HIRSEL_NATIVE_CPU_HARNESS': ['harness'],
                'HIRSEL_TEST_BIN_EXES': 'hirsel=compiler'}}
            with patch.object(runner, 'ROOT', root):
                runner.freeze_command(data, prepared, 'ordinary', {})
            for name in ['harness', 'compiler']:
                (root / name).write_text('sanitized')
            (root / 'products/index').write_text('sanitized')
            self.assertEqual((root / data['env']['HIRSEL_PRODUCTS'] / 'index').read_text(), 'ordinary')
            self.assertEqual(data['env']['HIRSEL_TEST_PRODUCTS'], data['env']['HIRSEL_PRODUCTS'])
            self.assertEqual((root / data['command'][1]).read_text(), 'ordinary')
            compiler = data['env']['HIRSEL_TEST_BIN_EXES'].split('=', 1)[1]
            self.assertEqual((root / compiler).read_text(), 'ordinary')
            self.assertEqual(data['env']['HIRSEL_NATIVE_CPU_HARNESS'], data['command'][1])

    def test_nonexclusive_failure_is_not_a_pass(self):
        failed, calls = self.invoke(status=101)
        self.assertTrue(failed)
        self.assertEqual(len(calls), 2)
        unit = calls[0].args[0][calls[0].args[0].index('--unit') + 1]
        self.assertEqual(calls[1].args[0], ['systemctl', '--user', 'stop', unit + '.scope'])

    def test_zero_executed_bodies_are_not_a_pass(self):
        failed, _ = self.invoke(stdout="test result: ok. 0 passed; 0 failed; 0 ignored;\n")
        self.assertTrue(failed)

    def test_scope_carries_exact_affinity_and_existing_memory_cap(self):
        failed, calls = self.invoke()
        self.assertFalse(failed)
        command = calls[0].args[0]
        self.assertIn('AllowedCPUs=0,1,2,3', command)
        self.assertIn('CPUQuota=400%', command)
        self.assertIn('MemoryMax=2097152K', command)
        self.assertIn('MemorySwapMax=0', command)
        self.assertIn('--include-ignored', command)
        self.assertEqual(calls[0].kwargs['env']['HIRSEL_CPU_PRIVATE_SCOPE'], 'inside')
        self.assertEqual(calls[0].kwargs['env']['HIRSEL_RUNTIME_ARCHIVES'], 'runtime-archive')
        self.assertIn('--exact', command)

    def test_timeout_stops_only_the_scope_the_runner_created(self):
        failed, calls = self.invoke(timeout=True)
        self.assertTrue(failed)
        command = calls[0].args[0]
        unit = command[command.index('--unit') + 1]
        self.assertEqual(calls[1].args[0], ['systemctl', '--user', 'stop', unit + '.scope'])


if __name__ == '__main__':
    unittest.main()
