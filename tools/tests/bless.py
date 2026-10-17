#!/usr/bin/env python3
"""Bless resolves current generated targets before either test invocation."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
BUCK = r'''#!/usr/bin/env python3
import json, os, re, sys
from pathlib import Path
args = sys.argv[1:]
if args[0] == 'uquery':
    pattern = re.search(r'attrregexfilter\(name, "([^"]+)"', args[1])[1]
    for name in json.loads(os.environ['TARGETS']):
        if re.fullmatch(pattern, name): print('root//:' + name)
else:
    with Path('calls.jsonl').open('a') as output:
        print(json.dumps(args), file=output)
    print('test result: ok. 1 passed; 0 failed; 0 ignored')
'''


class CurrentTargets(unittest.TestCase):
    def run_bless(self, what, targets):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / 'tools').mkdir()
            (root / '.buck2/bin').mkdir(parents=True)
            shutil.copyfile(ROOT / 'tools/bless.sh', root / 'tools/bless.sh')
            buck = root / '.buck2/bin/buck2'
            buck.write_text(BUCK)
            buck.chmod(0o755)
            subprocess.run(['git', 'init', '-q', str(root)], check=True)
            result = subprocess.run(['bash', 'tools/bless.sh', what], cwd=root,
                                    env=os.environ | {'TARGETS': json.dumps(targets)},
                                    capture_output=True, text=True)
            calls = root / 'calls.jsonl'
            return result, [json.loads(line) for line in calls.read_text().splitlines()] if calls.exists() else []

    def test_native_inventory_follows_sharding_changes(self):
        base = 'hirsel-pipeline-tests-test-front'
        for targets in ([base], [base + '-shard-1', base + '-shard-2']):
            with self.subTest(targets=targets):
                result, calls = self.run_bless('native-inventory', targets)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(len(calls), 2)
                for call in calls:
                    self.assertIn('root//:' + targets[0], call)
                    self.assertIn('--test-arg=rt_operations::declaration_digests_match_the_reviewed_native_inventory', call)
                self.assertIn('HIRSEL_BLESS=1', calls[0])
                self.assertNotIn('HIRSEL_BLESS=1', calls[1])

    def test_missing_or_ambiguous_targets_fail_before_blessing(self):
        base = 'hirsel-pipeline-tests-test-front'
        for targets in ([], [base, base + '-shard-1']):
            with self.subTest(targets=targets):
                result, calls = self.run_bless('native-inventory', targets)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn('expected one test target', result.stderr)
                self.assertEqual(calls, [])


if __name__ == '__main__':
    unittest.main()
