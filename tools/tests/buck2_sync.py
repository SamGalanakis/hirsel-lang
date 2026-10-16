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


class FullGateHostCases(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.main = self.root / 'main.rs'
        self.main.write_text('')

    def cases(self, text):
        (self.root / 'matrix.rs').write_text(text)
        return sync.host_cases('fixture-test-it', self.root, self.main)

    def test_helper_matrix_routes_full_gate_siblings_only(self):
        self.assertEqual(self.cases('''
fn campaign() { full_matrix(); }
#[test]
#[ignore = "full gate"]
fn full_gate_both_targets() { campaign(); }
#[test]
#[ignore = "full gate"]
fn full_gate_x86_control() { campaign(); }
#[test]
fn smoke() {}
#[test]
#[ignore]
fn manual_native() {}
'''), ['full_gate_both_targets', 'full_gate_x86_control'])

    def test_each_host_dependency_routes_a_new_full_gate_case(self):
        for dependency in [
            'full_matrix()', 'TargetSpec::aarch64_unknown_linux_gnu()',
            '&tools.aarch64_cc', '&tools.qemu_aarch64',
            'Command::new("aarch64-linux-gnu-gcc")',
            'Command::new("qemu-aarch64-static")', 'Command::new("valgrind")',
            'Command::new("lean")', 'Command::new("lake")',
            'Command::new("/usr/bin/valgrind")', 'Target::Aarch64',
            'use hirsel_driver::native::full_matrix as configurations;',
            '#[ignore = "full gate: host tools"]',
        ]:
            with self.subTest(dependency=dependency):
                self.assertEqual(self.cases(dependency + '\nfn full_gate_added() {}'), ['full_gate_added'])

    def test_pool_only_full_gate_cases_stay_on_pool(self):
        self.assertEqual(self.cases('fn full_gate_heap_only() { heap_campaign(); }'), [])

    def test_inline_module_test_name_is_a_libtest_filter(self):
        self.assertEqual(self.cases('mod nested { fn full_gate_nested() { full_matrix(); } }'), ['full_gate_nested'])

    def test_existing_host_filters_are_preserved(self):
        self.assertEqual(sync.host_cases('hirsel-rt-test-it', self.root, self.main), ['abi::'])

    def test_current_cross_target_full_gate_modules_are_hosted(self):
        root = sync.ROOT / 'crates/hirsel-pipeline-tests/tests/front'
        cases = sync.host_cases('hirsel-pipeline-tests-test-front', root, root / 'main.rs')
        for module in ('rt_io_native', 'rt_text_native'):
            for case in sync.FULL_GATE_CASE.findall((root / f'{module}.rs').read_text()):
                self.assertIn(case, cases)


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
