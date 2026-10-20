#!/usr/bin/env python3
"""Every ignored test runs in a gate.

An ignored test is a `full_gate_` case (`#[ignore = "full gate..."]`, run by
both full-gate crate stages of tools/main-runs), a release-only case
(`cfg_attr(debug_assertions, ignore ...)`, run by the release tests), or a case
in a module that tools/check-native.sh runs with its ignored tests. Expectation
rewrites are not ignored tests: they run under HIRSEL_BLESS (tools/bless.sh).
"""
from pathlib import Path
import re
import unittest

ROOT = Path(__file__).resolve().parents[2]
# Sources whose ignored cases tools/check-native.sh runs, with the stage's selection.
NATIVE = [
    ('crates/hirsel-llvm/src/', '//:hirsel-llvm-unittest //:hirsel-llvm-test-it -- "${timeout[@]}" $(libtest --include-ignored)'),
    ('crates/hirsel-llvm/tests/it/', '//:hirsel-llvm-unittest //:hirsel-llvm-test-it -- "${timeout[@]}" $(libtest --include-ignored)'),
    ('crates/hirsel-lir/tests/it/layout.rs', '//:hirsel-lir-test-it -- "${timeout[@]}" $(libtest layout:: --ignored)'),
    ('crates/hirsel-driver/tests/it/native.rs', '$(libtest native:: --include-ignored --test-threads=4)'),
    ('crates/hirsel-pipeline-tests/tests/rc/rc_library_', '$(libtest rc_library_ --include-ignored --test-threads=4)'),
    ('crates/hirsel-pipeline-tests/tests/rc/rc_maps_native.rs', '$(libtest rc_maps_native:: --include-ignored --test-threads=1)'),
]
ATTRIBUTE = re.compile(r'^[ \t]*#\[(ignore\b[^\]]*)\]|^[ \t]*#\[cfg_attr\(\s*([^,]+),\s*ignore\b', re.M)
FUNCTION = re.compile(r'\bfn\s+(\w+)')


def orphans(sources):
    """`(path, test)` for each ignored test of `{path: text}` no gate runs."""
    found = []
    for path, text in sorted(sources.items()):
        for attribute in ATTRIBUTE.finditer(text):
            name = FUNCTION.search(text, attribute.end())[1]
            if attribute[1] is not None:
                if name.startswith('full_gate_') and attribute[1].startswith('ignore = "full gate'):
                    continue
            elif attribute[2].strip() == 'debug_assertions':
                continue
            if any(path.startswith(prefix) for prefix, _ in NATIVE):
                continue
            found.append((path, name))
    return found


class IgnoredTests(unittest.TestCase):
    def test_every_ignored_test_in_the_workspace_runs_in_a_gate(self):
        sources = {str(p.relative_to(ROOT)): p.read_text() for p in ROOT.glob('crates/**/*.rs')}
        self.assertEqual(orphans(sources), [])

    def test_each_native_home_is_a_check_native_selection(self):
        script = (ROOT / 'tools/check-native.sh').read_text()
        for prefix, selection in NATIVE:
            self.assertIn(selection, script, prefix)
            self.assertTrue(list(ROOT.glob(prefix + '*')), prefix)

    def test_orphans_are_named_and_homes_are_not(self):
        sources = {
            'crates/a/tests/it/x.rs': '#[test]\n#[ignore = "full gate"]\nfn full_gate_a() {}\n'
                                      '#[test]\n#[ignore = "full gate"]\nfn misnamed() {}\n'
                                      '#[test]\n#[ignore]\nfn tool() {}\n'
                                      '#[test]\n#[cfg_attr(debug_assertions, ignore = "release")]\nfn release() {}\n'
                                      '#[test]\n#[cfg_attr(miri, ignore)]\nfn miri() {}\n'
                                      '#[test]\n#[ignore = "full gate"]\n#[should_panic]\nfn full_gate_b() {}\n',
            'crates/hirsel-lir/tests/it/layout.rs': '#[test]\n#[ignore = "native"]\nfn probe() {}\n',
        }
        self.assertEqual(orphans(sources), [('crates/a/tests/it/x.rs', 'misnamed'),
                                            ('crates/a/tests/it/x.rs', 'tool'),
                                            ('crates/a/tests/it/x.rs', 'miri')])


if __name__ == '__main__':
    unittest.main()
