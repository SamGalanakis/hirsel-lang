#!/usr/bin/env python3
"""Incremental attribution invariants; main-runs also discovers this script."""
import importlib.machinery
import importlib.util
from pathlib import Path
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
loader = importlib.machinery.SourceFileLoader('incremental_profile_9', str(ROOT/'tools/compiler-bench/incremental-profile-9'))
spec = importlib.util.spec_from_loader(loader.name, loader)
h = importlib.util.module_from_spec(spec)
loader.exec_module(h)


class Invariants(unittest.TestCase):
    def test_environment_excludes_unrelated_secrets(self):
        self.assertEqual(h.compiler_environment({'PATH':'/bin', 'HIRSEL_JOBS':'1', 'LLVM_SYS_231_PREFIX':'/llvm',
                                                'CONNECTOR_API_KEY':'private'}),
                         {'PATH':'/bin', 'HIRSEL_JOBS':'1', 'LLVM_SYS_231_PREFIX':'/llvm'})

    def test_edit_restores_exact_bytes_after_failure(self):
        with tempfile.TemporaryDirectory() as temp:
            source = Path(temp)/'main.hir'
            original = b'original\r\n'
            source.write_bytes(original)
            with self.assertRaisesRegex(RuntimeError, 'build failed'):
                with h.edit(source, 'edited'):
                    self.assertEqual(source.read_text(), 'edited')
                    raise RuntimeError('build failed')
            self.assertEqual(source.read_bytes(), original)

    def test_controls_change_one_site_and_body_preserves_signature(self):
        base = (h.FIXTURE/'main.hir').read_text()
        edits = h.variants(base)
        self.assertEqual(set(edits), {'unchanged', 'marker', 'body', 'signature'})
        self.assertEqual(edits['unchanged'], base)
        self.assertEqual(edits['body'].replace('x + 2', 'x + 1'), base)
        self.assertIn('helper(x:u64)->u64', edits['body'])
        self.assertEqual(edits['signature'].replace('helper(x:u32)->u32', 'helper(x:u64)->u64'), base)
        self.assertIn('let value=helper(4)', edits['body'])

    def test_cache_namespaces_are_unique_and_refuse_reuse(self):
        with tempfile.TemporaryDirectory() as temp:
            paths = [h.namespace(Path(temp), mode, n, edit) for mode in ('resident','disk','native')
                     for n in range(2) for edit in ('unchanged','body')]
            self.assertEqual(len(set(paths)), 12)
            with self.assertRaises(FileExistsError):
                h.namespace(Path(temp), 'disk', 0, 'body')

    def test_failed_and_incomplete_groups_are_excluded(self):
        passing = [{'status':'pass', 'perf':{'instructions':42}} for _ in range(10)]
        self.assertTrue(h.accepted(passing, 10))
        self.assertFalse(h.accepted(passing[:-1], 10))
        self.assertFalse(h.accepted(passing[:-1]+[{'status':'failed'}], 10))
        self.assertFalse(h.accepted([{'status':'pass'}]*10, 10))

    def test_freeze_detects_changed_missing_and_untracked_inputs(self):
        with tempfile.TemporaryDirectory() as temp:
            source = Path(temp)/'untracked'
            source.write_text('before')
            frozen = h.inventory([Path(temp)])
            h.verify(frozen, [Path(temp)])
            added = Path(temp)/'added'
            added.write_text('new input')
            with self.assertRaisesRegex(RuntimeError, 'tree membership changed'):
                h.verify(frozen, [Path(temp)])
            added.unlink()
            source.write_text('after')
            with self.assertRaisesRegex(RuntimeError, 'frozen inputs changed'):
                h.verify(frozen)
            source.unlink()
            with self.assertRaisesRegex(RuntimeError, 'frozen inputs changed'):
                h.verify(frozen)

    def test_missing_pmu_evidence_fails_closed(self):
        with tempfile.TemporaryDirectory() as temp:
            counters = Path(temp)/'perf'
            counters.write_text('<not counted>,,instructions,0,0\n')
            with self.assertRaises(ValueError):
                h.perf_counts(counters)
            counters.write_text('42,,instructions,100,100.00\n')
            with self.assertRaisesRegex(RuntimeError, 'incomplete PMU'):
                h.perf_counts(counters)


if __name__ == '__main__':
    unittest.main()
