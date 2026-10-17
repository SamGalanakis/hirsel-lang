#!/usr/bin/env python3
"""Keep full-gate tool dependencies in Buck2's host test partition."""
import importlib.util
from pathlib import Path
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'buck2'))
SPEC = importlib.util.spec_from_file_location('buck2_sync', Path(__file__).resolve().parents[1] / 'buck2/sync.py')
sync = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(sync)


class NativePoolRouting(unittest.TestCase):
    def test_no_campaign_implicitly_selects_the_host(self):
        self.assertTrue(sync.GATE_TESTS)
        for stage in sync.GATE_TESTS:
            self.assertFalse(stage.get('host', False), stage['name'])

    def test_filtered_native_entry_has_no_global_cover_dependency(self):
        stages = {stage['name']: stage for stage in sync.GATE_TESTS}
        self.assertNotIn('native_selection', stages['gate-native-full'])
        for index in range(1, sync.NATIVE_FULL_SHARDS + 1):
            self.assertEqual(stages[f'gate-native-full-shard-{index}']['native_selection'],
                             '//:native-selection')

    def test_generated_tests_have_no_host_partition(self):
        buck = (sync.ROOT / 'BUCK').read_text()
        self.assertNotIn('host_cases =', buck)
        self.assertNotIn('host = True', buck)


class SeededTests(unittest.TestCase):
    def test_a_binary_drawing_random_cases_anywhere_in_its_modules_is_seeded(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            main = root / 'main.rs'
            main.write_text('mod deep;\n')
            (root / 'deep').mkdir()
            (root / 'deep/mod.rs').write_text('#[test]\nfn fixed() {}\n')
            self.assertFalse(sync.seeded(root, main))
            (root / 'deep/mod.rs').write_text('proptest! {\n #[test]\n fn any(x in 0u8..) {}\n}\n')
            self.assertTrue(sync.seeded(root, main))

    def test_the_seed_reaches_exactly_the_proptest_binaries(self):
        import re
        buck = (Path(__file__).resolve().parents[2] / 'BUCK').read_text()
        seeded = {m[1] for m in re.finditer(r'hirsel_test\(\n    name = "([^"]+)",(.*?)\n\)', buck, re.S)
                  if 'seeded = True' in m[2]}
        self.assertIn('hirsel-parser-test-it', seeded)
        self.assertIn('hirsel-pipeline-tests-test-front', seeded)
        self.assertNotIn('hirsel-driver-test-it', seeded)


if __name__ == '__main__':
    unittest.main()
