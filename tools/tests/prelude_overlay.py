#!/usr/bin/env python3
"""The prelude overlay patches only the stock files its pins name, and only where its anchors are."""
import ast
import importlib.util
from pathlib import Path
import subprocess
import tempfile
import unittest

SPEC = importlib.util.spec_from_file_location('prelude_overlay', Path(__file__).resolve().parents[1] / 'buck2/prelude_overlay.py')
overlay = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(overlay)

# Each fixture file holds its stock anchors inside unrelated prelude text.
STOCK = {
    'tests/re_utils.bzl': 'def executor():\n    local_enabled = re_props_copy.pop("local_enabled", False)\n'
                          '    default = CommandExecutorConfig(\n        remote_enabled = True,\n    )\n'
                          '    if listing:\n        other = CommandExecutorConfig(\n            remote_enabled = True,\n        )\n',
    'rust/rust_library.bzl': 'def doctest():\n        rustdoc_test_info = ExternalRunnerTestInfo(\n            type = "rustdoc",\n            default_executor = re_executors.default_executor,\n        )\n',
    'rust/build.bzl': ''.join((
        'def compile(ctx, compile_ctx):\n'
        '    cmd_args(\n'
        '        hidden = compile_ctx.transitive_srcs.project_as_args("artifacts") if compile_ctx else [],\n'
        '    )\n\n'
        'def symlinked_dirs(ctx):\n',
        overlay.STOCK_DEPENDENCY_DIRS,
        '\ndef rustdoc(ctx, compile_ctx, toolchain_info):\n'
        '    cmd_args(\n'
        '        hidden = [toolchain_info.compiler, compile_ctx.transitive_srcs.project_as_args("artifacts")],\n'
        '    )\n',
    )),
    'rust/failure_filter.bzl': 'def failure_filter(ctx, compile_ctx):\n' + overlay.STOCK_FAILURE_FILTER,
    'utils/cmd_script.bzl': 'def cmd_script():\n    wrapper = [\n        "#!/usr/bin/env bash",\n    ]\n',
    'rust/sources.bzl': 'def _get_artifacts(sources):\n    return [sources]\n' + overlay.STOCK_SOURCES_TSET,
}


class PreludeOverlayGuards(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.prelude = Path(temp.name)
        for relative, text in STOCK.items():
            self.write(relative, text)
        self.pins = {
            'inputs': {relative: overlay.digest(text.encode()) for relative, text in STOCK.items()},
            'outputs': {relative: overlay.digest(overlay.transform(relative, text).encode()) for relative, text in STOCK.items()},
        }

    def write(self, relative, text):
        path = self.prelude / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text)

    def test_pinned_stock_files_are_patched_and_patched_files_verify(self):
        outputs = overlay.overlaid(self.prelude, self.pins)
        build = outputs['rust/build.bzl'].decode()
        self.assertNotIn('category = "deps"', build)
        self.assertIn('ctx.actions.assembled_dir(transitive_dependency_dir', build)
        self.assertEqual(build.count('project_as_args("checkout_artifacts")'), 2)
        self.assertNotIn('project_as_args("artifacts")', build)
        self.assertIn('ctx.actions.dynamic_output(', outputs['rust/failure_filter.bzl'].decode())
        self.assertIn('owner.package == "third-party"', outputs['rust/sources.bzl'].decode())
        self.assertIn('supports_test_execution_caching = True', outputs['rust/rust_library.bzl'].decode())
        self.assertEqual(outputs['tests/re_utils.bzl'].decode().count('priority = priority,'), 2)
        self.assertIn('TEST_TARGET', outputs['rust/rust_library.bzl'].decode())
        for relative, data in outputs.items():
            self.write(relative, data.decode())
        self.assertEqual(overlay.overlaid(self.prelude, self.pins), outputs)

    def test_linker_wrapper_runs_without_path_and_preserves_arguments(self):
        def run_wrapper(source):
            shebang = next(ast.literal_eval(line.strip().rstrip(','))
                           for line in source.splitlines() if '"#!' in line)
            wrapper = self.prelude / 'linker_wrapper.sh'
            wrapper.write_text(shebang + "\n/bin/printf '%s\\n' \"$@\"\n")
            wrapper.chmod(0o755)
            return subprocess.run([str(wrapper), 'one argument', 'two'], env={'PATH': ''},
                                  capture_output=True, text=True)

        stock = run_wrapper(STOCK['utils/cmd_script.bzl'])
        self.assertEqual(stock.returncode, 127)
        patched = run_wrapper(overlay.overlaid(self.prelude, self.pins)['utils/cmd_script.bzl'].decode())
        self.assertEqual(patched.returncode, 0, patched.stderr)
        self.assertEqual(patched.stdout, 'one argument\ntwo\n')

    def test_a_file_matching_neither_pin_is_refused(self):
        self.write('rust/failure_filter.bzl', STOCK['rust/failure_filter.bzl'] + '\n')
        with self.assertRaisesRegex(overlay.OverlayError, r'pinned prelude input changed: rust/failure_filter\.bzl'):
            overlay.overlaid(self.prelude, self.pins)

    def test_a_pinned_input_without_its_anchor_is_refused(self):
        # A re-pinned prelude whose stock text moved: the checksum passes and the anchor does not.
        moved = STOCK['rust/build.bzl'].replace('category = "deps"', 'category = "dependency_dirs"')
        self.write('rust/build.bzl', moved)
        self.pins['inputs']['rust/build.bzl'] = overlay.digest(moved.encode())
        with self.assertRaisesRegex(overlay.OverlayError, r'stock dependency directory action not found exactly once'):
            overlay.overlaid(self.prelude, self.pins)

    def test_a_transform_that_was_not_re_pinned_is_refused(self):
        self.pins['outputs']['rust/sources.bzl'] = overlay.digest(b'an earlier overlay')
        with self.assertRaisesRegex(overlay.OverlayError, r'overlay output does not match pins\.json: rust/sources\.bzl'):
            overlay.overlaid(self.prelude, self.pins)


if __name__ == '__main__':
    unittest.main()
