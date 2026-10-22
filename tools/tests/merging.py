#!/usr/bin/env python3
"""Corpus ratchets accept real lane append shapes without losing removals."""
from pathlib import Path
import sys
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'buck2'))
import merging


class PathLists(unittest.TestCase):
    def test_unsorted_appends_next_to_comments_and_twin_rows_keep_both_lanes(self):
        base = 'conformance/Z/existing.hir\n# new lane cases\n'
        a = base + 'conformance/B/ok.hir\nconformance/B/bad.hir\n'
        b = base + 'conformance/A/ok.hir\nconformance/A/bad.hir\n'
        for left, right in [(a, b), (b, a)]:
            text = merging.resolve('conformance/passing.txt', base, left, right)
            self.assertIsNotNone(text)
            self.assertEqual(set(text.splitlines()), set((a + b).splitlines()))
            entries = [line for line in text.splitlines() if not line.startswith('#')]
            self.assertEqual(entries, sorted(entries))

    def test_removal_is_not_resurrected_when_other_lane_adds(self):
        base = 'conformance/A/old.hir\nconformance/Z/keep.hir\n'
        a = 'conformance/Z/keep.hir\nconformance/B/new.hir\n'
        b = base + '# append\nconformance/C/new.hir\n'
        text = merging.resolve('conformance/passing.txt', base, a, b)
        self.assertNotIn('old.hir', text)
        self.assertIn('conformance/B/new.hir', text)
        self.assertIn('conformance/C/new.hir', text)

    def test_diverging_expectations_and_delete_versus_edit_stay_conflicts(self):
        base = 'CAP/a.hir # old reason\n'
        a, b = 'CAP/a.hir # new reason\n', 'CAP/a.hir # other reason\n'
        for left, right in [(a, b), ('', b)]:
            self.assertIsNone(merging.resolve('crates/hirsel-authority/src/corpus/pending.txt',
                                              base, left, right))

    def test_duplicate_or_non_path_entries_are_never_hidden(self):
        row = 'conformance/A/new.hir\n'
        for invalid in [row + row, '../outside.hir\n', 'not a path\n']:
            self.assertIsNone(merging.resolve('conformance/passing.txt', '', invalid, row))


class LineSets(unittest.TestCase):
    def test_every_row_obeys_three_way_set_membership(self):
        # Exhaust all eight base/ours/theirs membership combinations, including
        # deletion against unchanged, shared additions and both deletions.
        from itertools import product
        combinations = list(product([False, True], repeat=3))
        texts = [''.join(f'row{i}\n' for i, flags in enumerate(combinations) if flags[side])
                 for side in range(3)]
        expected = {f'row{i}' for i, (base, ours, theirs) in enumerate(combinations)
                    if (base and ours and theirs) or (ours and not base) or (theirs and not base)}
        for path in ['tools/known-failures.tsv', 'crates/example/tests/it/modules.rs']:
            with self.subTest(path=path):
                self.assertEqual(set(merging.resolve(path, *texts).splitlines()), expected)

    def test_unsorted_lines_keep_order_and_normalize_missing_final_newline(self):
        self.assertEqual(merging.resolve('tools/known-failures.tsv', 'R\nkeep',
                                         'keep\nzed', 'R\nkeep\nalpha'), 'keep\nzed\nalpha\n')


if __name__ == '__main__':
    unittest.main()
