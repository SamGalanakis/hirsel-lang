#!/usr/bin/env python3
"""Archive links must resolve within the pinned archive, including Debian usrmerge."""
import importlib.util
from pathlib import Path
import shutil
import tempfile
import sys
import unittest

sys.dont_write_bytecode = True

SPEC = importlib.util.spec_from_file_location('unpack_native', Path(__file__).resolve().parents[1] / 'buck2/native-toolchain/unpack.py')
unpack = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(unpack)


class ArchiveLinks(unittest.TestCase):
    def test_absolute_usrmerge_loader_comes_from_the_archive(self):
        with tempfile.TemporaryDirectory() as raw:
            root = Path(raw) / 'archive'
            library = root / 'usr/lib/x86_64-linux-gnu/ld-linux-x86-64.so.2'
            library.parent.mkdir(parents=True)
            library.write_bytes(b'pinned loader')
            loader = root / 'usr/lib64/ld-linux-x86-64.so.2'
            loader.parent.mkdir()
            loader.symlink_to('/lib/x86_64-linux-gnu/ld-linux-x86-64.so.2')
            unpack.normalize_links(root)
            output = Path(raw) / 'output'
            unpack.copy_tree(root / 'usr', output)
            copied = output / 'lib64/ld-linux-x86-64.so.2'
            self.assertFalse(copied.is_symlink())
            self.assertEqual(copied.read_bytes(), b'pinned loader')

    def test_relative_in_archive_link_is_accepted_and_escape_is_rejected(self):
        with tempfile.TemporaryDirectory() as raw:
            root = Path(raw) / 'archive'
            root.mkdir()
            (root / 'tool').write_bytes(b'pinned tool')
            (root / 'alias').symlink_to('tool')
            unpack.normalize_links(root)
            self.assertEqual((root / 'alias').read_bytes(), b'pinned tool')
            (root / 'escape').symlink_to('../../outside')
            with self.assertRaisesRegex(ValueError, 'escapes its root'):
                unpack.normalize_links(root)


if __name__ == '__main__':
    unittest.main()
