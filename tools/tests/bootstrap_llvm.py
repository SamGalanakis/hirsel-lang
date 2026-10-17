#!/usr/bin/env python3
"""The pinned LLVM wrappers run without discovering tools through PATH."""
import importlib.util
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

SCRIPTS = Path(__file__).resolve().parents[1] / 'buck2'
sys.path.insert(0, str(SCRIPTS))
SPEC = importlib.util.spec_from_file_location('bootstrap_llvm', SCRIPTS / 'bootstrap_llvm.py')
llvm = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(llvm)


class LlvmWrappers(unittest.TestCase):
    def test_wrappers_preserve_arguments_and_loader_path_without_path(self):
        with tempfile.TemporaryDirectory(prefix='hirsel wrapper ') as raw:
            tree = Path(raw) / 'tree with spaces'
            (tree / 'bin').mkdir(parents=True)
            for name in ('clang', 'clang++', 'llvm-ar'):
                with self.subTest(tool=name):
                    tool = tree / 'bin' / name
                    tool.write_text('#!/bin/sh\nprintf \'%s\\n\' "$LD_LIBRARY_PATH" "$@"\n')
                    tool.chmod(0o755)
                    wrapper = tree / 'bin' / f'hirsel-{name}'
                    wrapper.write_text(llvm.WRAPPER % (name, name))
                    wrapper.chmod(0o755)
                    result = subprocess.run([str(wrapper.relative_to(raw)), 'one argument', 'two'],
                                            cwd=raw, env={'PATH': '', 'LD_LIBRARY_PATH': '/inherited libs'},
                                            capture_output=True, text=True)
                    self.assertEqual(result.returncode, 0, result.stderr)
                    self.assertEqual(result.stdout, f'{tree}/lib:/inherited libs\none argument\ntwo\n')


if __name__ == '__main__':
    unittest.main()
