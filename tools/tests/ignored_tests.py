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
    ('crates/hirsel-cli/tests/cli/ordered_qualification.rs', '//:hirsel-cli-test-cli-shard-1 -- "${long[@]}" $(libtest ordered_qualification:: runtime50_ordered:: --ignored --test-threads=1)'),
    ('crates/hirsel-cli/tests/cli/runtime50_ordered.rs', '//:hirsel-cli-test-cli-shard-1 -- "${long[@]}" $(libtest ordered_qualification:: runtime50_ordered:: --ignored --test-threads=1)'),
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
    ('crates/hirsel-artifact-store/src/index/tests/races.rs', 'cooperating_child'):
        ('multiprocess_common_pin_publication_and_live_staging_gc_owner', 'child'),
    ('crates/hirsel-parser/tests/it/grammar_fuzz_22.rs', 'worker'):
        ('bounded_grammar_recovery', 'isolated'),
    ('crates/hirsel-cli/tests/cli/policy_evaluator_33.rs', 'policy_evaluator_33_cell'):
        ('policy_evaluator_33_simulation', 'campaign'),
}
REPLAY_ACTIONS = {
    ('crates/hirsel-cli/tests/cli/ordered_deadline_liveness_55.rs', 'fixture'),
    ('crates/hirsel-cli/tests/cli/ordered_deadline_liveness_55.rs', 'cell'),
}
# These Buck proof targets supply their own immutable tool inputs, rather than
# the archival runner's environment. Each probe checks exact identity/counts.
PROOFS = {
    'native::object_queries::platform::closure_tests::selected_virtual_inputs_keep_scripts_thin_members_and_absolute_dso_names': {
        'source': 'crates/hirsel-driver/tests/native-validation/platform_closure.rs',
        'target': '//tools/buck2/native-toolchain:platform-closure-proof',
        'probe': 'tools/buck2/native-toolchain/platform_closure_proof.py',
        'arguments': ['/original pinned llvm', '/original packaged linker', '/selected kit', '/selected runtime', '/selected auditors', '/selected floor', '/selected qemu', '/selected harness'],
        'environment': {'LLVM_SYS_231_PREFIX': '/original pinned llvm',
                        'HIRSEL_CAPTURE_LINKER': '/original packaged linker',
                        'HIRSEL_LINK_KIT': '/selected kit', 'HIRSEL_RUNTIME_ARCHIVES': '/selected runtime',
                        'HIRSEL_AUDITOR_PACKAGE': '/selected auditors', 'HIRSEL_GLIBC_FLOOR': '/selected floor',
                        'HIRSEL_QEMU_AARCH64': '/selected qemu/qemu-aarch64'},
    },
    'requests::native_bootstrap::tests::actual_native_bootstrap_links_both_backends_and_restores_after_runtime_drop': {
        'source': 'crates/hirsel-driver/src/requests/native_bootstrap/tests.rs',
        'target': '//tools/buck2/native-toolchain:native-bootstrap-link-proof',
        'probe': 'tools/buck2/native-toolchain/native_bootstrap_link_proof.py',
        'arguments': ['/original pinned llvm', '/original packaged linker', '/selected kit', '/selected runtime', '/selected auditors', '/selected floor', '/selected qemu', '/selected harness'],
        'environment': {'LLVM_SYS_231_PREFIX': '/original pinned llvm',
                        'HIRSEL_CAPTURE_LINKER': '/original packaged linker',
                        'HIRSEL_LINK_KIT': '/selected kit', 'HIRSEL_RUNTIME_ARCHIVES': '/selected runtime',
                        'HIRSEL_AUDITOR_PACKAGE': '/selected auditors', 'HIRSEL_GLIBC_FLOOR': '/selected floor',
                        'HIRSEL_QEMU_AARCH64': '/selected qemu/qemu-aarch64'},
    },
    'requests::native_bootstrap::tests::actual_native_bootstrap_uses_selected_closed_runtime_on_link_and_reopen': {
        'source': 'crates/hirsel-driver/src/requests/native_bootstrap/tests.rs',
        'target': '//tools/buck2/native-toolchain:native-closed-bootstrap-proof',
        'probe': 'tools/buck2/native-toolchain/native_closed_bootstrap_proof.py',
        'arguments': ['/original pinned llvm', '/original packaged linker', '/selected kit', '/selected runtime', '/selected auditors', '/selected floor', '/selected qemu', '/selected harness'],
        'environment': {'LLVM_SYS_231_PREFIX': '/original pinned llvm',
                        'HIRSEL_CAPTURE_LINKER': '/original packaged linker',
                        'HIRSEL_LINK_KIT': '/selected kit', 'HIRSEL_RUNTIME_ARCHIVES': '/selected runtime',
                        'HIRSEL_AUDITOR_PACKAGE': '/selected auditors', 'HIRSEL_GLIBC_FLOOR': '/selected floor',
                        'HIRSEL_QEMU_AARCH64': '/selected qemu/qemu-aarch64'},
    },
    'native::object_queries::platform::tests::selected_platform_closes_runtime_before_keys_and_replays_without_originals': {
        'source': 'crates/hirsel-driver/tests/native-validation/platform_capture.rs',
        'target': '//tools/buck2/native-toolchain:platform-capture-proof',
        'probe': 'tools/buck2/native-toolchain/platform_capture_proof.py',
        'arguments': ['/original pinned llvm', '/original packaged linker', '/selected kit', '/selected runtime', '/selected auditors', '/selected floor', '/selected qemu', '/selected harness'],
        'environment': {'LLVM_SYS_231_PREFIX': '/original pinned llvm',
                        'HIRSEL_CAPTURE_LINKER': '/original packaged linker',
                        'HIRSEL_LINK_KIT': '/selected kit', 'HIRSEL_RUNTIME_ARCHIVES': '/selected runtime',
                        'HIRSEL_AUDITOR_PACKAGE': '/selected auditors', 'HIRSEL_GLIBC_FLOOR': '/selected floor',
                        'HIRSEL_QEMU_AARCH64': '/selected qemu/qemu-aarch64'},
    },
    'requests::native_bootstrap::tests::actual_native_bootstrap_binds_run_identity_and_delivers_live_output': {
        'source': 'crates/hirsel-driver/src/requests/native_bootstrap/tests.rs',
        'target': '//tools/buck2/native-toolchain:native-bound-bootstrap-proof',
        'probe': 'tools/buck2/native-toolchain/native_bound_bootstrap_proof.py',
        'arguments': ['/original pinned llvm', '/original packaged linker', '/selected kit', '/selected runtime', '/selected auditors', '/selected floor', '/selected qemu', '/selected harness'],
        'environment': {'LLVM_SYS_231_PREFIX': '/original pinned llvm',
                        'HIRSEL_CAPTURE_LINKER': '/original packaged linker',
                        'HIRSEL_LINK_KIT': '/selected kit', 'HIRSEL_RUNTIME_ARCHIVES': '/selected runtime',
                        'HIRSEL_AUDITOR_PACKAGE': '/selected auditors', 'HIRSEL_GLIBC_FLOOR': '/selected floor',
                        'HIRSEL_QEMU_AARCH64': '/selected qemu/qemu-aarch64'},
    },
    'requests::native_selection::tests::actual_native_selector_coalesces_build_and_bound_run_without_a_retained_controller': {
        'source': 'crates/hirsel-driver/src/requests/native_selection/tests.rs',
        'target': '//tools/buck2/native-toolchain:native-selector-proof',
        'probe': 'tools/buck2/native-toolchain/native_selector_proof.py',
        'arguments': ['/original pinned llvm', '/original packaged linker', '/selected kit', '/selected runtime', '/selected auditors', '/selected floor', '/selected qemu', '/selected harness'],
        'environment': {'LLVM_SYS_231_PREFIX': '/original pinned llvm',
                        'HIRSEL_CAPTURE_LINKER': '/original packaged linker',
                        'HIRSEL_LINK_KIT': '/selected kit', 'HIRSEL_RUNTIME_ARCHIVES': '/selected runtime',
                        'HIRSEL_AUDITOR_PACKAGE': '/selected auditors', 'HIRSEL_GLIBC_FLOOR': '/selected floor',
                        'HIRSEL_QEMU_AARCH64': '/selected qemu/qemu-aarch64'},
    },
    'native::object_queries::capture::tests::packaged_capture_replays_changed_inputs_without_originals': {
        'source': 'crates/hirsel-driver/tests/native-validation/link_capture.rs',
        'target': '//tools/buck2/native-toolchain:linker-store-proof',
        'probe': 'tools/buck2/native-toolchain/linker_store_probe.py',
    },
    'native::kit::tests::stored_linker_replays_shared_executable_plans_for_both_targets': {
        'source': 'crates/hirsel-driver/src/native/kit.rs',
        'target': '//tools/buck2/native-toolchain:executable-plan-proof',
        'probe': 'tools/buck2/native-toolchain/executable_plan_proof.py',
        'arguments': ['/original pinned llvm', '/original packaged linker', '/selected kit', '/selected runtime', '/selected harness'],
        'environment': {'LLVM_SYS_231_PREFIX': '/original pinned llvm',
                        'HIRSEL_CAPTURE_LINKER': '/original packaged linker',
                        'HIRSEL_LINK_KIT': '/selected kit', 'HIRSEL_RUNTIME_ARCHIVES': '/selected runtime'},
    },
    'native::kit::tests::frozen_kit_binds_transitive_leaf_edits_and_plans_without_originals': {
        'source': 'crates/hirsel-driver/src/native/kit.rs',
        'target': '//tools/buck2/native-toolchain:frozen-kit-proof',
        'probe': 'tools/buck2/native-toolchain/frozen_kit_proof.py',
    },
    'native::object_queries::capture::tests::large_tool_snapshots_and_replays_under_none_and_unwritable_with_original_ram_cap': {
        'source': 'crates/hirsel-driver/tests/native-validation/link_capture.rs',
        'target': '//tools/buck2/native-toolchain:scratch-linker-proof',
        'probe': 'tools/buck2/native-toolchain/scratch_linker_proof.py',
    },
    'native::object_queries::runtime::tests::actual_runtime_products_reopen_changed_leaf_versions_and_refuse_missing_closure': {
        'source': 'crates/hirsel-driver/tests/native-validation/runtime_products.rs',
        'target': '//tools/buck2/native-toolchain:runtime-store-proof',
        'probe': 'tools/buck2/native-toolchain/runtime_store_proof.py',
        'arguments': ['/original pinned llvm', '/selected auditor', '/selected floor', '/selected qemu', '/selected harness'],
        'environment': {'LLVM_SYS_231_PREFIX': '/original pinned llvm', 'HIRSEL_AUDITOR_PACKAGE': '/selected auditor',
                        'HIRSEL_GLIBC_FLOOR': '/selected floor', 'HIRSEL_QEMU_AARCH64': '/selected qemu/qemu-aarch64'},
    },
    'native::object_queries::runtime::tests::equal_bytes_keep_distinct_loaded_libraries_and_shared_aliases': {
        'source': 'crates/hirsel-driver/tests/native-validation/runtime_products.rs',
        'target': '//tools/buck2/native-toolchain:runtime-identity-case-proof',
        'probe': 'tools/buck2/native-toolchain/runtime_identity_case_proof.py',
        'arguments': ['/original pinned llvm', '/selected auditor', '/selected floor', '/selected qemu', '/selected harness'],
        'environment': {'LLVM_SYS_231_PREFIX': '/original pinned llvm', 'HIRSEL_AUDITOR_PACKAGE': '/selected auditor',
                        'HIRSEL_GLIBC_FLOOR': '/selected floor', 'HIRSEL_QEMU_AARCH64': '/selected qemu/qemu-aarch64'},
    },
    'native::object_queries::runtime::tests::actual_loader_discovers_transitive_runtime_inputs_and_closes_replay': {
        'source': 'crates/hirsel-driver/tests/native-validation/runtime_products.rs',
        'target': '//tools/buck2/native-toolchain:runtime-discovery-proof',
        'probe': 'tools/buck2/native-toolchain/runtime_discovery_proof.py',
        'arguments': ['/original pinned llvm', '/selected auditor', '/selected floor', '/selected qemu', '/selected harness'],
        'environment': {'LLVM_SYS_231_PREFIX': '/original pinned llvm', 'HIRSEL_AUDITOR_PACKAGE': '/selected auditor',
                        'HIRSEL_GLIBC_FLOOR': '/selected floor', 'HIRSEL_QEMU_AARCH64': '/selected qemu/qemu-aarch64'},
    },
    'native_compilation_tasks_19::frozen_root_plans_run_both_backends_without_live_linker_inputs': {
        'source': 'crates/hirsel-driver/tests/it/native_compilation_tasks_19.rs',
        'target': '//tools/buck2/native-toolchain:root-plan-proof',
        'probe': 'tools/buck2/native-toolchain/root_plan_proof.py',
        'arguments': ['/original pinned llvm', '/original packaged linker', '/selected kit', '/selected runtime', '/selected harness'],
        'environment': {'LLVM_SYS_231_PREFIX': '/original pinned llvm',
                        'HIRSEL_CAPTURE_LINKER': '/original packaged linker',
                        'HIRSEL_LINK_KIT': '/selected kit', 'HIRSEL_RUNTIME_ARCHIVES': '/selected runtime'},
    },
    'native_compilation_tasks_19::admitted_root_plans_bind_the_kit_before_codegen': {
        'source': 'crates/hirsel-driver/tests/it/native_compilation_tasks_19.rs',
        'target': '//tools/buck2/native-toolchain:root-frozen-kit-proof',
        'probe': 'tools/buck2/native-toolchain/root_frozen_kit_proof.py',
        'arguments': ['/original pinned llvm', '/original packaged linker', '/selected kit', '/selected runtime', '/selected harness'],
        'environment': {'LLVM_SYS_231_PREFIX': '/original pinned llvm',
                        'HIRSEL_CAPTURE_LINKER': '/original packaged linker',
                        'HIRSEL_LINK_KIT': '/selected kit', 'HIRSEL_RUNTIME_ARCHIVES': '/selected runtime'},
    },
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


def ignored_declarations(text):
    """Resolve simple fn-name macro templates to their actual invocation names."""
    for attribute in ATTRIBUTE.finditer(text):
        function = re.search(r'\bfn\s+(\$?\w+)', text[attribute.end():])
        name = function[1]
        if name.startswith('$') and attribute[1] is not None:
            macro = re.findall(r'macro_rules!\s+(\w+)', text[:attribute.start()])[-1]
            names = re.findall(r'\b' + re.escape(macro) + r'!\s*\(\s*(\w+)', text)
            if not names:
                raise ValueError(f'ignored macro has no registered invocations: {macro}')
            for name in names:
                yield attribute, name
        else:
            yield attribute, name


def orphans(sources):
    """`(path, test)` for each ignored test of `{path: text}` without a home."""
    found = []
    for path, text in sorted(sources.items()):
        for attribute, name in ignored_declarations(text):
            if attribute[1] is not None:
                if name.startswith('full_gate_') and attribute[1].startswith('ignore = "full gate'):
                    continue
            elif attribute[2].strip() == 'debug_assertions':
                continue
            if (path, name) in DELEGATED:
                continue
            if (path, name) in REPLAY_ACTIONS:
                continue
            if any(home['source'] == path and case.rsplit('::', 1)[-1] == name
                   for case, home in PROOFS.items()):
                continue
            home = next((home for case, home in EXPLICIT.items()
                         if home['source'] == path and case.rsplit('::', 1)[-1] == name), None)
            if home:
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
            if helper == 'child':
                self.assertIn('"index::tests::races::cooperating_child"', helper_body)
                command_body = text.split('fn test_command(', 1)[1].split('\nfn ', 1)[0]
                self.assertIn('current_exe()', command_body)
                self.assertIn('["--exact", name,', command_body)
                self.assertIn('TestChild::spawn(', helper_body)
                self.assertIn('self.process.wait()', text)
                self.assertIn('test result: ok. 1 passed; 0 failed;', text)
                self.assertIn('"--ignored"', helper_body)
                for key in ('INDEX36_CHILD_ROOT', 'INDEX36_CHILD_MODE', 'INDEX36_CHILD_ID'):
                    self.assertIn(key, helper_body)
                self.assertIn('child(', parent_body)
                self.assertIn('.spawn()', text.split('impl TestChild {', 1)[1].split('impl Drop', 1)[0])
                self.assertIn('a.finish()', parent_body)
                self.assertIn('b.finish()', parent_body)
                self.assertIn('mod races;', (ROOT / Path(path).parent / 'modules.rs').read_text())
            elif helper == 'isolated':
                self.assertIn('current_exe()', helper_body)
                self.assertRegex(helper_body, rf'"--exact",\s+"{module}::{worker}",\s+"--ignored"')
                self.assertIn('child.wait()', helper_body)
            else:
                self.assertIn('current_exe()', helper_body)
                self.assertRegex(helper_body, rf'"--exact",\s+"{module}::{worker}",\s+"--ignored"')
                self.assertIn('.output()', helper_body)
                self.assertIn('output.status.success()', helper_body)
                self.assertIn('"--cpu=60:60"', helper_body)
                self.assertIn('.env("HIRSEL_MEMORY_LIMIT", "2G")', helper_body)
            self.assertTrue(any(FUNCTION.search(text, a.end())[1] == worker
                                for a in ATTRIBUTE.finditer(text)))
            # The annotation covers only the named child, never a sibling.
            sources = {path: '#[test]\n#[ignore]\nfn unregistered_worker() {}\n'}
            self.assertEqual(orphans(sources), [(path, 'unregistered_worker')])

    def test_deadline_replay_actions_select_exact_workers_and_supply_their_inputs(self):
        definitions = (ROOT / 'tools/buck2/defs.bzl').read_text()
        for path, name in REPLAY_ACTIONS:
            self.assertIn(name, [name for _, name in ignored_declarations((ROOT / path).read_text())])
            self.assertIn(f'args = ["--ignored", "--exact", "ordered_deadline_liveness_55::{name}", "--nocapture"]', definitions)
            self.assertEqual(orphans({path: '#[ignore]\nfn unregistered() {}'}), [(path, 'unregistered')])
        for supplied in ['env["HIRSEL_REPLAY_FIXTURE_OUT"] = cmd_args(fixture.as_output())',
                         'files_env = {"HIRSEL_REPLAY_FIXTURE": ":" + fixture}',
                         'extra_env = {"HIRSEL_REPLAY_BACKEND": backend, "HIRSEL_REPLAY_CELL": cell}']:
            self.assertIn(supplied, definitions)
        self.assertIn('call("hirsel_deadline_replay_matrix", name="deadline-select-replay"',
                      (ROOT / 'tools/buck2/sync.py').read_text())

    def test_explicit_homes_execute_exact_ignored_identity_and_forward_required_inputs(self):
        runner = runpy.run_path(str(ROOT / 'tools/ignored-tests'))
        self.assertTrue((ROOT / 'tools/ignored-tests').stat().st_mode & 0o111)
        for case, home in EXPLICIT.items():
            text = (ROOT / home['source']).read_text()
            names = [name for _, name in ignored_declarations(text)]
            self.assertIn(case.rsplit('::', 1)[-1], names, case)
            source = Path(home['source'])
            if home.get('standalone', False):
                self.assertEqual(source.parent.name, 'tests')
                self.assertEqual(home['target'], f'//:{source.parents[1].name}-test-{source.stem}')
                self.assertNotIn('::', case)
            else:
                registry = home.get('module_registry', str(source.parent / 'modules.rs'))
                self.assertIn(f'mod {source.stem};', (ROOT / registry).read_text())
            environment = {key: '/retained/input with spaces' for key in home['inputs']}
            command = runner['command'](case, environment)
            self.assertEqual(command[-5:], ['--test-arg=--exact', f'--test-arg={case}',
                                           '--test-arg=--ignored', '--test-arg=--test-threads=1',
                                           '--test-arg=--nocapture'])
            self.assertEqual(command[1:command.index('--') + 1], ['test', '-c', 'hirsel.tests=host',
                                           home.get('target', '//:hirsel-cli-test-cli-shard-1'),
                                           *[argument for value in home.get('config', []) for argument in ['-c', value]], '--'])
            for value in home.get('config', []):
                self.assertIn(value, command)
                self.assertEqual(command[command.index(value) - 1], '-c')
            for key in home['inputs']:
                self.assertIn(f'{key}={environment[key]}', command)
                with self.assertRaises(ValueError):
                    runner['command'](case, environment | {key: ''})
            # Registration never covers an adjacent ignored case in the same file.
            self.assertEqual(orphans({home['source']: '#[ignore]\nfn unregistered() {}'}),
                             [(home['source'], 'unregistered')])

    def test_buck_proof_homes_forward_tools_and_require_one_exact_execution(self):
        for case, home in PROOFS.items():
            self.assertIn(case.rsplit('::', 1)[-1], [name for _, name in ignored_declarations((ROOT / home['source']).read_text())])
            self.assertEqual(orphans({home['source']: '#[ignore]\nfn unregistered() {}'}), [(home['source'], 'unregistered')])
            self.assertIn('tests/native-validation/link_capture.rs', (ROOT / 'crates/hirsel-driver/src/native/object_queries/capture.rs').read_text())
            target = (ROOT / 'tools/buck2/native-toolchain/BUCK').read_text()
            self.assertIn(f'name = "{home["target"].rsplit(":", 1)[-1]}"', target)
            self.assertIn(f'script = "{Path(home["probe"]).name}", interpreter = "python3"', target)
            declaration = next(line for line in target.splitlines()
                               if f'name = "{home["target"].rsplit(":", 1)[-1]}"' in line)
            tools = re.search(r'tools = \[([^\]]*)\]', declaration)[1]
            self.assertEqual(len(re.findall(r'"[^"]+"', tools)), len(home.get('arguments', [0, 0, 0])), case)
            probe = runpy.run_path(str(ROOT / home['probe']))
            arguments = home.get('arguments', ['/original packaged linker', '/original pinned llvm', '/selected harness'])
            selected, environment = probe['command'](arguments, {'KEEP': 'original'})
            self.assertEqual(selected, [arguments[-1], '--ignored', '--exact', case, '--test-threads=1', '--nocapture'])
            forwarded = home.get('environment', {'HIRSEL_CAPTURE_LINKER': arguments[0], 'LLVM_SYS_231_PREFIX': arguments[1]})
            for key, value in forwarded.items():
                self.assertEqual(environment[key], value)
            self.assertEqual(environment['LD_LIBRARY_PATH'], '/original pinned llvm/lib')
            self.assertEqual(environment['KEEP'], 'original')
            for status, output, expected in [
                (0, f'test {case} ... ok\ntest result: ok. 1 passed; 0 failed; 0 ignored;', 0),
                (0, 'test result: ok. 0 passed; 0 failed; 0 ignored;', 1),
                (0, 'test result: ok. 0 passed; 0 failed; 1 ignored;', 1),
                (0, 'test other ... ok\ntest result: ok. 1 passed; 0 failed; 0 ignored;', 1),
                (0, f'test {case} ... ok\ntest result: ok. 2 passed; 0 failed; 0 ignored;', 1),
                (32, 'test result: FAILED. 0 passed; 1 failed; 0 ignored;', 32),
            ]:
                with patch.object(subprocess, 'run', return_value=types.SimpleNamespace(returncode=status, stdout=output)) as run, patch('builtins.print'):
                    self.assertEqual(probe['main'](arguments), expected)
                    self.assertEqual(run.call_args.args[0], selected)
                    for key, value in forwarded.items():
                        self.assertEqual(run.call_args.kwargs['env'][key], value)

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

    def test_ignored_macro_templates_name_invocations_instead_of_the_next_helper(self):
        text = '\n'.join([
            'macro_rules! controls { ($name:ident) => {',
            '#[test]', '#[ignore = "manual"]', 'fn $name() {}', '} }',
            'controls!(first);', 'controls!(second);', 'fn ordinary_helper() {}'])
        self.assertEqual([name for _, name in ignored_declarations(text)], ['first', 'second'])
        self.assertEqual(orphans({'crates/example/controls.rs': text}), [
            ('crates/example/controls.rs', 'first'), ('crates/example/controls.rs', 'second')])

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
