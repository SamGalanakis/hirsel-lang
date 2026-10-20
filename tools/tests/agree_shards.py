#!/usr/bin/env python3
"""agree.py refuses an agreement campaign with a missing, duplicate or
non-covering shard, and the native merge keeps a stopped shard's clusters but
judges stale known kinds only when every shard reported."""
import json
from pathlib import Path
import sys
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'buck2'))
import agree  # noqa: E402

SAMPLE = dict(seed=1, limit=agree.UNLIMITED, hash_seed=0, programs=2, generated=0, eligible=6,
              variants_per_program=3)


def report(index, count, checked=2, clusters=()):
    value = dict(SAMPLE, shard=dict(index=index, count=count), checked=checked, skipped=0, rejected=1,
                 valid=checked - 1, unavailable=0, runtime_limits=0, executed=1, edits={'sendable': checked},
                 executions={'run': 1}, native_agreed=1, clusters=list(clusters))
    return 'noise before the report\n' + json.dumps(value) + '\n'


class Summarize(unittest.TestCase):
    def test_every_shard_once_sums_and_a_missing_duplicate_or_short_shard_refuses(self):
        outputs = {f'shard-{i}': report(i, 3) for i in (1, 2, 3)}
        total = agree.summarize(outputs, shards=3)
        self.assertEqual((total['checked'], total['valid'], total['edits']), (6, 3, {'sendable': 6}))
        refused = {
            'missing': {k: v for k, v in outputs.items() if k != 'shard-3'},
            'duplicate': dict(outputs, **{'shard-3': report(2, 3)}),
            'out of range': dict(outputs, **{'shard-3': report(4, 3)}),
            'short of the sample': dict(outputs, **{'shard-3': report(3, 3, checked=1)}),
            'flagged': dict(outputs, **{'shard-3': report(3, 3, clusters=[{'kind': 'k', 'edit': 'e'}])}),
        }
        for name, case in refused.items():
            with self.subTest(name), self.assertRaises(ValueError):
                agree.summarize(case, shards=3)

    def test_native_merge_keeps_streamed_clusters_and_judges_known_kinds_only_when_complete(self):
        cluster = {'kind': 'native/heap/Leak', 'edit': 'sendable'}
        stopped = 'agree: cluster ' + json.dumps(cluster) + '\n'
        merged, errors = agree.merge_native({'a': report(1, 2), 'b': stopped}, 2, {'native/heap/Old'})
        self.assertEqual(merged['clusters'], [cluster])
        self.assertTrue(any('missing [2]' in e for e in errors), errors)
        self.assertTrue(any('new discrepancy kind: native/heap/Leak' in e for e in errors), errors)
        self.assertFalse(any('no longer occurs' in e for e in errors), errors)
        _, errors = agree.merge_native({'a': report(1, 2, checked=3), 'b': report(2, 2, checked=3)}, 2,
                                       {'native/heap/Old'})
        self.assertEqual(errors, ['native/heap/Old no longer occurs; delete it from '
                                  'native-tests/regressions/agree-known.txt'])
        _, errors = agree.merge_native({'a': report(1, 2), 'b': report(1, 2)}, 2, set())
        self.assertTrue(any('duplicate' in e for e in errors) and any('missing [2]' in e for e in errors), errors)


if __name__ == '__main__':
    unittest.main()
