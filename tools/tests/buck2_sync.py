#!/usr/bin/env python3
"""Check Buck2 inventories and exclusive temporary-file publication."""
import importlib.util
import contextlib
import io
from pathlib import Path
import sys
import tempfile
import unittest
import json
import subprocess
import threading
from concurrent.futures import ThreadPoolExecutor
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'buck2'))
SPEC = importlib.util.spec_from_file_location('buck2_sync', Path(__file__).resolve().parents[1] / 'buck2/sync.py')
sync = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(sync)


class AtomicScratch(unittest.TestCase):
    def concurrent_writes(self, writer, output):
        barrier = threading.Barrier(2)
        replace = Path.replace

        def publish(path, destination):
            barrier.wait(timeout=10)
            return replace(path, destination)

        with patch.object(Path, 'replace', publish), ThreadPoolExecutor(2) as workers:
            futures = [workers.submit(writer, i) for i in range(2)]
            for future in futures:
                future.result()
        self.assertTrue(output.is_file())
        self.assertEqual(list(output.parent.iterdir()), [output])

    def test_inventory_writers_use_distinct_exclusive_temporary_files(self):
        with tempfile.TemporaryDirectory() as scratch:
            output = Path(scratch) / 'BUCK'
            self.concurrent_writes(lambda i: sync.write(output, str(i)), output)
            self.assertIn(output.read_text(), ['0', '1'])

    def test_stamp_writers_use_distinct_exclusive_temporary_files(self):
        with tempfile.TemporaryDirectory() as scratch:
            root = Path(scratch)
            reads = sync.generated.Reads(root)
            output = root / sync.generated.STAMP
            self.concurrent_writes(lambda _: sync.generated.record(root, 'first-party', reads), output)
            self.assertIn('first-party', json.loads(output.read_text())['parts'])


class GeneratedStamp(unittest.TestCase):
    """generated.py's stamp: a part is current while every output is as written
    and everything its generation read reads the same."""

    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name).resolve()
        (self.root / 'src').mkdir()
        (self.root / 'src/lib.rs').write_text('fn f() {}\n')
        (self.root / 'README.md').write_text('notes\n')
        for part in sync.generated.PARTS:
            self.generate(part)

    def generate(self, part):
        reads = sync.generated.Reads(self.root)
        reads.text(self.root / 'src/lib.rs')
        reads.is_file(self.root / 'src/main.rs')
        reads.glob(self.root / 'src', '**/*.rs')
        for path in sync.generated.PARTS[part]:
            (self.root / path).parent.mkdir(parents=True, exist_ok=True)
            (self.root / path).write_text(path + '\n')
        sync.generated.record(self.root, part, reads)

    def test_a_part_is_current_until_a_read_a_probe_a_glob_or_an_output_changes(self):
        stale = lambda: sync.generated.stale(self.root)  # noqa: E731
        self.assertEqual(stale(), [])
        (self.root / 'README.md').write_text('more notes\n')
        self.assertEqual(stale(), [], 'generation never read it')
        for change, undo, parts in [
                (lambda: (self.root / 'src/lib.rs').write_text('fn g() {}\n'),
                 lambda: (self.root / 'src/lib.rs').write_text('fn f() {}\n'), list(sync.generated.PARTS)),
                (lambda: (self.root / 'src/main.rs').write_text(''), lambda: (self.root / 'src/main.rs').unlink(),
                 list(sync.generated.PARTS)),
                (lambda: (self.root / 'src/deep').mkdir() or (self.root / 'src/deep/x.rs').write_text(''),
                 lambda: (self.root / 'src/deep/x.rs').unlink() or (self.root / 'src/deep').rmdir(),
                 list(sync.generated.PARTS)),
                (lambda: (self.root / 'BUCK').write_text('edited\n'), lambda: (self.root / 'BUCK').write_text('BUCK\n'),
                 ['first-party']),
                (lambda: (self.root / 'third-party/BUCK').unlink(),
                 lambda: (self.root / 'third-party/BUCK').write_text('third-party/BUCK\n'), ['third-party'])]:
            change()
            self.assertEqual(stale(), parts)
            undo()
            self.assertEqual(stale(), [])

    def test_a_file_that_changes_while_generation_reads_it_leaves_the_part_stale(self):
        reads = sync.generated.Reads(self.root)
        reads.text(self.root / 'src/lib.rs')
        (self.root / 'src/lib.rs').write_text('fn g() {}\n')
        reads.text(self.root / 'src/lib.rs')
        sync.generated.record(self.root, 'first-party', reads)
        for content in ('fn f() {}\n', 'fn g() {}\n'):
            (self.root / 'src/lib.rs').write_text(content)
            self.assertIn('first-party', sync.generated.stale(self.root))

    def test_a_lane_change_touching_a_generated_file_is_refused_with_the_commands_that_drop_it(self):
        self.assertEqual(sync.generated.refusal(['README.md', 'crates/a/src/lib.rs']), [])
        refused = '\n'.join(sync.generated.refusal(['docs/api.md', 'BUCK', 'README.md']))
        self.assertIn('no lane commits: BUCK, docs/api.md', refused)
        self.assertIn('git checkout $(git merge-base HEAD origin/main) -- docs/api.md', refused)
        self.assertIn('git rm -q --cached BUCK', refused)


class GeneratedParts(unittest.TestCase):
    """The real generation on a copy of HEAD's workspace: each part reruns for
    exactly its inputs, and an unchanged output is never rewritten."""

    @classmethod
    def setUpClass(cls):
        cls.temp = tempfile.TemporaryDirectory(prefix='hirsel-sync-')
        cls.root = Path(cls.temp.name).resolve()
        archive = subprocess.run(['git', 'archive', 'HEAD', '--', 'Cargo.toml', 'Cargo.lock', 'rust-toolchain.toml',
                                  'crates', 'proofs', 'conformance', 'third-party', 'tools/buck2', 'tools/native-toolchain',
                                  'README.md'], cwd=sync.ROOT, check=True, capture_output=True).stdout
        subprocess.run(['tar', '-x', '-C', str(cls.root)], input=archive, check=True)
        cls.addClassCleanup(cls.temp.cleanup)
        cls.patches = [patch.object(sync, 'ROOT', cls.root), patch.object(sync, 'HERE', cls.root / 'tools/buck2'),
                       patch.object(sync, 'THIRD_PARTY', cls.root / 'third-party'),
                       patch.object(sync, 'COSTS', cls.root / 'tools/buck2/test-costs')]
        for active in cls.patches:
            active.start()
            cls.addClassCleanup(active.stop)
        cls.sync()

    @classmethod
    def tearDownClass(cls):
        for active in cls.patches:
            active.stop()
        cls.temp.cleanup()

    @classmethod
    def sync(cls, *flags):
        """Run sync.py; returns the parts it regenerated."""
        with patch.object(sys, 'argv', ['sync.py', *flags]), patch.object(sync, 'generate', wraps=sync.generate) as run:
            assert sync.main() == 0
        return list(run.call_args.args[1]) if run.called else []

    def setUp(self):
        self.assertEqual(sync.generated.stale(self.root), [])
        self.written = self.identities()

    def identities(self):
        return {path: (lambda found: (found.st_ino, found.st_mtime_ns))((self.root / path).stat())
                for path in sync.generated.BUILD_INPUTS}

    def rewritten(self):
        return sorted(path for path, identity in self.identities().items() if identity != self.written[path])

    def edit(self, path, text):
        """Write a file, restoring it (and the generated files) after the test."""
        file = self.root / path
        before = file.read_bytes() if file.exists() else None
        self.addCleanup(self.sync, '--if-stale')
        self.addCleanup(lambda: file.write_bytes(before) if before is not None else file.unlink())
        file.parent.mkdir(parents=True, exist_ok=True)
        file.write_text(text)

    def buck(self):
        return (self.root / 'BUCK').read_text()

    def test_a_documentation_or_corpus_program_edit_regenerates_nothing(self):
        self.edit('README.md', 'A different readme\n')
        self.edit('docs/notes.md', 'notes\n')
        program = next((self.root / 'conformance').rglob('*.hir')).relative_to(self.root)
        self.edit(program, '// edited\n')
        self.assertEqual(self.sync('--if-stale'), [])

    def test_a_manifest_edit_regenerates_both_parts_and_rewrites_only_what_changed(self):
        manifest = (self.root / 'Cargo.toml').read_text()
        old = '[profile.dev.package.hirsel-span]\nopt-level = 1\n'
        self.assertIn(old, manifest)
        self.assertIn('    opt_level = 1,\n    name = "hirsel-span",\n', self.buck())
        self.edit('Cargo.toml', manifest.replace(old, '[profile.dev.package.hirsel-span]\nopt-level = 2\n'))
        self.assertEqual(self.sync('--if-stale'), list(sync.generated.PARTS))
        self.assertIn('    opt_level = 2,\n    name = "hirsel-span",\n', self.buck())
        self.assertEqual(self.rewritten(), ['BUCK'])

    def test_an_added_test_file_adds_its_target_without_rerunning_reindeer(self):
        self.edit('crates/hirsel-span/tests/added.rs', 'hirsel_guard::install!();\n\n#[test]\nfn added() {}\n')
        self.assertEqual(self.sync('--if-stale'), ['first-party'])
        self.assertIn('    name = "hirsel-span-test-added",\n', self.buck())
        self.assertEqual(self.rewritten(), ['BUCK'])

    def test_unit_costs_split_actions_without_changing_the_compiled_library(self):
        library = self.library('hirsel-driver')
        self.edit('tools/buck2/test-costs/hirsel-driver-unittest.tsv',
                  'first::case\t240.0\nsecond::case\t240.0\n')
        self.assertEqual(self.sync('--if-stale'), ['first-party'])
        block = next(block.split('\n)\n')[0] for block in self.buck().split('hirsel_test(\n')[1:]
                     if '    name = "hirsel-driver-unittest",\n' in block.split('\n)\n')[0])
        self.assertIn('    shards = 2,\n', block)
        self.assertIn('    costs = "//tools/buck2:test-costs/hirsel-driver-unittest.tsv",\n', block)
        self.assertEqual(self.library('hirsel-driver'), library)

    def library(self, name):
        """The crate's hirsel_library call in the root BUCK."""
        return next(block.split('\n)\n')[0] for block in self.buck().split('hirsel_library(\n')[1:]
                    if f'    name = "{name}",\n' in block.split('\n)\n')[0])

    def test_a_doc_comment_code_block_turns_on_the_crates_doctests(self):
        crate = next(path for path in sorted((self.root / 'crates').glob('*/src/lib.rs'))
                     if 'doctests' not in self.library(path.parents[1].name))
        self.edit(crate.relative_to(self.root), crate.read_text() + '\n/// ```\n/// assert!(true);\n/// ```\nfn documented() {}\n')
        self.assertEqual(self.sync('--if-stale'), ['first-party'])
        self.assertIn('doctests = True', self.library(crate.parents[1].name))


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

    def test_global_cover_is_opt_in_before_test_runner_filters_arrive(self):
        source = (sync.ROOT / 'tools/buck2/defs.bzl').read_text()
        code = source[source.index('def hirsel_command_test('):source.index('def _script_binary_impl(')]
        for settings, expected in [({}, None), ({'native_selection': 'true'}, '//:native-selection'),
                                    ({'native_selection': 'derive'}, '//:native-selection'),
                                    ({'native_selection_file': 'frozen.json'}, '//:native-selection')]:
            calls = []
            scope = dict(FULL_GATE=True, TESTS_ON_HOST=False, TEST_SEED='',
                         read_root_config=lambda section, key, default: settings.get(key, default),
                         native_tool_inputs=lambda: {}, _test_platform=lambda name: [],
                         _request=lambda name: [], _stage=lambda name: name,
                         _rust_test=lambda **kwargs: calls.append(kwargs))
            exec(code, scope)
            for name in ['gate-native-full-shard-1', 'gate-agree-native-full-shard-1']:
                scope['hirsel_command_test'](name, '//:binary', ['--full'], [], '.',
                                             full=True, native_selection='//:native-selection')
                self.assertEqual(calls[-1]['native_selection'], expected)

    def test_selected_conformance_forwards_every_prefix_in_each_slice(self):
        source = (sync.ROOT / 'tools/buck2/defs.bzl').read_text()
        code = source[source.index('def selected_paths('):source.index('def _script_binary_impl(')]
        real = 'conformance/OWN/OWN-8-local-aggregate-ok.hir'
        missing = 'conformance/definitely-not-a-case.hir'
        calls = []
        scope = dict(FULL_GATE=False, TESTS_ON_HOST=False, TEST_SEED='',
                     read_root_config=lambda section, key, default:
                         f'{real}|{missing};{real}|{real}' if key == 'conformance_paths' else default,
                     native_tool_inputs=lambda: {}, _test_platform=lambda name: [],
                     _request=lambda name: [], _stage=lambda name: name,
                     _rust_test=lambda **kwargs: calls.append(kwargs))
        exec(code, scope)
        for index, expected in [(1, [real, missing]), (2, [real, real])]:
            scope['hirsel_command_test'](f'gate-conformance-selected-shard-{index}',
                                         '//:binary', ['--check-format'], [], '.',
                                         selected=('conformance_paths', '--path', index))
            self.assertEqual(calls[-1]['args'], ['--check-format'] +
                             [arg for path in expected for arg in ['--path', path]])
        scope['hirsel_command_test']('gate-conformance-selected-shard-3', '//:binary',
                                     [], [], '.', selected=('conformance_paths', '--path', 3))
        self.assertEqual(len(calls), 2)

    def test_generated_tests_have_no_host_partition(self):
        buck = (sync.ROOT / 'BUCK').read_text()
        self.assertNotIn('host_cases =', buck)
        rust_tests = '\n'.join(block.split('\n)\n', 1)[0] for block in buck.split('hirsel_test(\n')[1:])
        self.assertNotIn('host = True', rust_tests)
        hosts = [block for block in buck.split('\n)\n') if 'host = True' in block]
        self.assertEqual(len(hosts), 1)
        self.assertIn('name = \"gate-free-genmc\"', hosts[0])


class GuardedRoots(unittest.TestCase):
    def test_a_root_or_a_path_module_installs_the_guard(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / 'plain.rs').write_text('//! A harness.\nuse std::fs;\n// hirsel_guard::install!();\n')
            (root / 'lib.rs').write_text('//! A library.\n#[cfg(test)]\nhirsel_guard::install!();\n')
            (root / 'support.rs').write_text('hirsel_guard::install!(Counting);\n')
            (root / 'main.rs').write_text('#[path = "support.rs"]\nmod support;\n')
            self.assertFalse(sync.guarded(root / 'plain.rs'))
            self.assertTrue(sync.guarded(root / 'lib.rs'))
            self.assertTrue(sync.guarded(root / 'main.rs'))


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


class ShardSizing(unittest.TestCase):
    def test_check_fails_on_cost_coverage_even_with_fresh_generated_files(self):
        with patch.object(sys, 'argv', ['sync.py', '--check']), \
             patch.object(sync, 'REINDEER', Path(__file__)), patch.object(sync.generated, 'stale', return_value=[]), \
             patch.object(sync, 'generate') as generate, \
             patch.object(sync, 'check_cost_coverage', return_value=['costs missing']), \
             contextlib.redirect_stderr(io.StringIO()) as errors:
            self.assertEqual(sync.main(), 1)
        self.assertIn('costs missing', errors.getvalue())
        generate.assert_not_called()


    def test_granted_cpus_determine_shards(self):
        with tempfile.TemporaryDirectory() as scratch:
            here = Path(scratch)
            binary = sync.SHARDED[0]
            (here / 'test-costs').mkdir()
            (here / f'test-costs/{binary}.tsv').write_text(''.join(f'case{i}\t40.0\n' for i in range(16)))
            for cpus, expected in [(1, 16), (2, 8), (4, 4)]:
                (here / 'test-sizes.json').write_text(json.dumps({binary: {'cpu_count': cpus}}))
                with patch.object(sync, 'HERE', here), patch.object(sync, 'COSTS', here / 'test-costs'):
                    self.assertEqual(sync.shards(binary)[0], expected)

    def test_expensive_cases_do_not_create_more_shards_than_measured_cases(self):
        # Each case already exceeds the shard work budget; extra partitions
        # cannot split it and only add empty remote actions to the main run.
        with tempfile.TemporaryDirectory() as scratch:
            here = Path(scratch)
            binary = sync.SHARDED[0]
            (here / 'test-costs').mkdir()
            (here / f'test-costs/{binary}.tsv').write_text('native_a\t1700.0\nnative_b\t650.0\n')
            (here / 'test-sizes.json').write_text(json.dumps({binary: {'cpu_count': 2}}))
            with patch.object(sync, 'HERE', here), patch.object(sync, 'COSTS', here / 'test-costs'):
                self.assertEqual(sync.shards(binary)[0], 2)

    def test_exactly_five_percent_missing_is_allowed_but_stale_rows_do_not_help(self):
        with tempfile.TemporaryDirectory() as scratch, patch.object(sync, 'SHARDED', ['binary']), \
             patch.object(sync, 'REQUIRED_COSTS', {'binary'}):
            table = Path(scratch) / 'binary.tsv'
            table.write_text(''.join(f'case{i}\t1\n' for i in range(19)) + 'deleted\t10000\n')
            with patch.object(sync, 'COSTS', Path(scratch)):
                self.assertEqual(sync.cost_coverage({'binary': {f'case{i}' for i in range(20)}}), [])
                errors = sync.cost_coverage({'binary': {f'case{i}' for i in range(21)}})
                self.assertIn('2/21 cases', errors[0])
                self.assertIn('case20', errors[0])
                self.assertIn('no case inventory', sync.cost_coverage({})[0])

    def test_unrefreshed_binary_warns_without_blocking_the_gate(self):
        binary = 'hirsel-rt-test-it'
        with patch.object(sync, 'SHARDED', [binary]), contextlib.redirect_stderr(io.StringIO()) as output:
            self.assertEqual(sync.cost_coverage({binary: {'new::case'}}), [])
        self.assertIn(f'warning: {binary}: 1/1 cases', output.getvalue())


if __name__ == '__main__':
    unittest.main()
