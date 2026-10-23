#!/usr/bin/env python3
"""Every ignored test has a registered executable home.

An ignored test is a `full_gate_` case (`#[ignore = "full gate..."]`, run by
both full-gate crate stages of tools/main-runs), a release-only case
(`cfg_attr(debug_assertions, ignore ...)`, run by the release tests), or a case
in a module that tools/check-native.sh runs with its ignored tests.
A registered subprocess worker runs through its ordinary bounded parent test.
Archival audits and host qualifications have exact tools/ignored-tests commands.
Expectation rewrites are not ignored tests: they run under HIRSEL_BLESS (tools/bless.sh).
"""
from pathlib import Path
import json
import re
import runpy
import shlex
import subprocess
import types
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
# Sources whose ignored cases tools/check-native.sh runs, with the stage's selection.
NATIVE = [
    ('crates/hirsel-cli/tests/cli/ordered_qualification.rs', '//:hirsel-cli-test-cli-shard-1 -- "${long[@]}" $(libtest ordered_qualification:: --ignored --test-threads=1)'),
    ('crates/hirsel-pipeline-tests/tests/rc/flat_storage_audit_16.rs', '$(libtest flat_storage_audit_16::native_flat_region_copy_parity --ignored --test-threads=1)'),
    ('crates/hirsel-pipeline-tests/tests/rc/native_desc_cell_16.rs', '$(libtest native_desc_cell_16:: --ignored --test-threads=1)'),
    ('crates/hirsel-pipeline-tests/tests/rc/native_simd_eq_17.rs', '$(libtest native_simd_eq_17:: --ignored --test-threads=1)'),
    ('crates/hirsel-pipeline-tests/tests/rc/isolate_bytes_leak_17.rs', '$(libtest isolate_bytes_leak_17::isolate_returns_unique_bytes --ignored --test-threads=1)'),
    ('crates/hirsel-pipeline-tests/tests/rc/rc_import.rs', '$(libtest rc_import::loop_version_matches_reference_heap_llvm_and_cranelift --ignored --test-threads=1)'),
    ('crates/hirsel-llvm/src/', '//:hirsel-llvm-unittest //:hirsel-llvm-test-it -- "${timeout[@]}" $(libtest --include-ignored)'),
    ('crates/hirsel-llvm/tests/it/', '//:hirsel-llvm-unittest //:hirsel-llvm-test-it -- "${timeout[@]}" $(libtest --include-ignored)'),
    ('crates/hirsel-lir/tests/it/layout.rs', '//:hirsel-lir-test-it -- "${timeout[@]}" $(libtest layout:: --ignored)'),
    ('crates/hirsel-driver/tests/it/native.rs', '$(libtest native:: --include-ignored --test-threads=4)'),
    ('crates/hirsel-pipeline-tests/tests/rc/rc_library_', '$(libtest rc_library_ --include-ignored --test-threads=4)'),
    ('crates/hirsel-pipeline-tests/tests/front/llvm_scalar_library.rs', '//:hirsel-pipeline-tests-test-front-shard-1 -- "${timeout[@]}" $(libtest llvm_scalar_library:: --include-ignored)'),
    ('crates/hirsel-pipeline-tests/tests/rc/rc_maps_native.rs', '$(libtest rc_maps_native:: --include-ignored --test-threads=1)'),
    ('crates/hirsel-pipeline-tests/tests/rc/unique_copy_nested_20.rs', '//:hirsel-pipeline-tests-test-rc-shard-1 -- "${long[@]}" $(libtest unique_copy_nested_20:: --ignored --test-threads=1)'),
    ('crates/hirsel-pipeline-tests/tests/rc/rc_borrow_cursor_8.rs', '//:hirsel-pipeline-tests-test-rc-shard-1 -- "${long[@]}" $(libtest rc_borrow_cursor_ --ignored --test-threads=1)'),
    ('crates/hirsel-pipeline-tests/tests/rc/rc_borrow_cursor_field_match_13.rs', '//:hirsel-pipeline-tests-test-rc-shard-1 -- "${long[@]}" $(libtest rc_borrow_cursor_ --ignored --test-threads=1)'),
    ('crates/hirsel-pipeline-tests/tests/rc/rc_borrow_cursor_compose_13.rs', '//:hirsel-pipeline-tests-test-rc-shard-1 -- "${long[@]}" $(libtest rc_borrow_cursor_ --ignored --test-threads=1)'),
]
# These workers require parent-supplied input and cannot run alone in a gate.
DELEGATED = {
    ('crates/hirsel-parser/tests/it/grammar_fuzz_22.rs', 'worker'):
        ('bounded_grammar_recovery', 'isolated'),
    ('crates/hirsel-cli/tests/cli/policy_evaluator_33.rs', 'policy_evaluator_33_cell'):
        ('policy_evaluator_33_simulation', 'campaign'),
}
EXPLICIT = json.loads((ROOT / 'tools/ignored-tests.json').read_text())
ATTRIBUTE = re.compile(r'^[ \t]*#\[(ignore\b[^\]]*)\]|^[ \t]*#\[cfg_attr\(\s*([^,]+),\s*ignore\b', re.M)
FUNCTION = re.compile(r'\bfn\s+(\w+)')


def native_covers(path, name, prefix, selection):
    """The source home must also select this libtest name, not just its file."""
    arguments = shlex.split(selection.split('$(libtest ', 1)[1].split(')', 1)[0])
    filters = [argument for argument in arguments if not argument.startswith('--')]
    qualified = f'{Path(path).stem}::{name}'
    return path.startswith(prefix) and (not filters or any(f in qualified for f in filters))


def orphans(sources):
    """`(path, test)` for each ignored test of `{path: text}` without a home."""
    found = []
    for path, text in sorted(sources.items()):
        for attribute in ATTRIBUTE.finditer(text):
            name = FUNCTION.search(text, attribute.end())[1]
            if attribute[1] is not None:
                if name.startswith('full_gate_') and attribute[1].startswith('ignore = "full gate'):
                    continue
            elif attribute[2].strip() == 'debug_assertions':
                continue
            if (path, name) in DELEGATED:
                continue
            home = EXPLICIT.get(f'{Path(path).stem}::{name}')
            if home and home['source'] == path:
                continue
            if any(native_covers(path, name, prefix, selection) for prefix, selection in NATIVE):
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

    def test_delegated_workers_have_an_unignored_parent_and_exact_child_selection(self):
        for (path, worker), (parent, helper) in DELEGATED.items():
            text = (ROOT / path).read_text()
            module = Path(path).stem
            self.assertRegex(text, rf'#\[test\]\s+fn {parent}\(\)')
            parent_body = text.split(f'fn {parent}()', 1)[1].split('\n#[test]', 1)[0]
            self.assertIn(f'{helper}(', parent_body)
            helper_body = text.split(f'fn {helper}(', 1)[1].split('\nfn ', 1)[0]
            self.assertRegex(helper_body, rf'"--exact",\s+"{module}::{worker}",\s+"--ignored"')
            self.assertIn('current_exe()', helper_body)
            if helper == 'isolated':
                self.assertIn('child.wait()', helper_body)
            else:
                self.assertIn('.output()', helper_body)
                self.assertIn('output.status.success()', helper_body)
                self.assertIn('"--cpu=60:61"', helper_body)
                self.assertIn('.env("HIRSEL_MEMORY_LIMIT", "2G")', helper_body)
            self.assertTrue(any(FUNCTION.search(text, a.end())[1] == worker
                                for a in ATTRIBUTE.finditer(text)))
            # The annotation covers only the named child, never a sibling.
            sources = {path: '#[test]\n#[ignore]\nfn unregistered_worker() {}\n'}
            self.assertEqual(orphans(sources), [(path, 'unregistered_worker')])

    def test_explicit_homes_execute_exact_ignored_identity_and_forward_required_inputs(self):
        runner = runpy.run_path(str(ROOT / 'tools/ignored-tests'))
        self.assertTrue((ROOT / 'tools/ignored-tests').stat().st_mode & 0o111)
        for case, home in EXPLICIT.items():
            text = (ROOT / home['source']).read_text()
            names = [FUNCTION.search(text, a.end())[1] for a in ATTRIBUTE.finditer(text)]
            self.assertIn(case.split('::')[1], names, case)
            self.assertIn(f'mod {Path(home["source"]).stem};',
                          (ROOT / 'crates/hirsel-cli/tests/cli/modules.rs').read_text())
            environment = {key: '/retained/input with spaces' for key in home['inputs']}
            command = runner['command'](case, environment)
            self.assertEqual(command[-5:], ['--test-arg=--exact', f'--test-arg={case}',
                                           '--test-arg=--ignored', '--test-arg=--test-threads=1',
                                           '--test-arg=--nocapture'])
            self.assertEqual(command[1:6], ['test', '-c', 'hirsel.tests=host',
                                           '//:hirsel-cli-test-cli-shard-1', '--'])
            for key in home['inputs']:
                self.assertIn(f'{key}={environment[key]}', command)
                with self.assertRaises(ValueError):
                    runner['command'](case, environment | {key: ''})
            # Registration never covers an adjacent ignored case in the same file.
            self.assertEqual(orphans({home['source']: '#[ignore]\nfn unregistered() {}'}),
                             [(home['source'], 'unregistered')])

    def test_cursor_home_selects_registered_modules_and_ignored_cases(self):
        registry = (ROOT / 'crates/hirsel-pipeline-tests/tests/rc/modules.rs').read_text()
        for prefix, selection in NATIVE:
            if '/rc_borrow_cursor_' not in prefix:
                continue
            path = Path(prefix)
            self.assertIn(f'mod {path.stem};', registry)
            self.assertIn('//:hirsel-pipeline-tests-test-rc-shard-1 --', selection)
            self.assertIn('--ignored', selection)
            text = (ROOT / path).read_text()
            names = [FUNCTION.search(text, attribute.end())[1] for attribute in ATTRIBUTE.finditer(text)]
            self.assertTrue(names, prefix)
            for name in names:
                self.assertTrue(native_covers(prefix, name, prefix, selection), name)
            # A registered source with the wrong libtest filter must still fail closed.
            wrong = selection.replace('libtest rc_borrow_cursor_', 'libtest unrelated::')
            self.assertFalse(native_covers(prefix, names[0], prefix, wrong))

    def test_explicit_runner_refuses_zero_execution_and_propagates_failures(self):
        runner = runpy.run_path(str(ROOT / 'tools/ignored-tests'))
        case = next(case for case, home in EXPLICIT.items() if not home['inputs'])
        for status, output, expected in [
                (0, 'test result: ok. 1 passed; 0 failed; 0 ignored;', 0),
                (0, 'test result: ok. 0 passed; 0 failed; 0 ignored;', 1),
                (0, 'test result: ok. 0 passed; 0 failed; 1 ignored;', 1),
                (0, '', 1),
                (32, 'test result: FAILED. 0 passed; 1 failed; 0 ignored;', 32)]:
            with patch('sys.argv', ['ignored-tests', case]), \
                 patch.object(subprocess, 'run', return_value=types.SimpleNamespace(
                     returncode=status, stdout=output)) as run, \
                 patch('builtins.print'):
                self.assertEqual(runner['main'](), expected)
                self.assertEqual(run.call_args.args[0], runner['command'](case, {}))

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
