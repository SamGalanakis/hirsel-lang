#!/usr/bin/env python3
"""tools/gate's selection, change identity and summary, and tools/land's loop,
against scratch repositories with a fake Buck2."""
import contextlib
import fcntl
import io
import json
import os
from pathlib import Path
import shutil
import selectors
import signal
import subprocess
import sys
import tempfile
import threading
import unittest
from unittest.mock import patch
from time import time_ns

sys.dont_write_bytecode = True
TOOLS = Path(__file__).resolve().parents[1]
REPO = TOOLS.parent
sys.path.insert(0, str(TOOLS / 'buck2'))
import gating  # noqa: E402
import impact  # noqa: E402
import merging  # noqa: E402
import graph  # noqa: E402

# Scratch gates must never contaminate the host's real timing history.
os.environ['HIRSEL_TEST_HISTORY'] = 'none'

MEMBERS = {'hirsel-lexer', 'hirsel-llvm', 'hirsel-cli', 'hirsel-types'}
BUCK = '''# @generated
load("//tools/buck2:defs.bzl", "hirsel_library", "hirsel_test")

hirsel_library(
    name = "hirsel-lexer",
    srcs = glob([
        "crates/hirsel-lexer/src/**",
    ]) + [
        "docs/grammar.md",
    ],
)

hirsel_test(
    name = "hirsel-lexer-test-it",
    data = glob([
        "conformance/**",
    ]),
)
'''


# The scratch clone's test target `a`, which reads the test code the Scripts change.
SCRIPT_BUCK = BUCK + '''
hirsel_test(
    name = "a",
    data = glob([
        "crates/hirsel-span/tests/**",
        "crates/hirsel-lexer/tests/**",
    ]),
)
'''


def git(root, *arguments):
    return subprocess.run(['git', '-C', str(root), *arguments], check=True, capture_output=True,
                          text=True).stdout.strip()


def scratch_repo(root):
    git(root, 'init', '-q', '-b', 'main')
    git(root, 'config', 'user.email', 'test@example.invalid')
    git(root, 'config', 'user.name', 'Gate test')


def commit(root, files, message):
    for path, text in files.items():
        (root / path).parent.mkdir(parents=True, exist_ok=True)
        if text is None:
            (root / path).unlink()
        else:
            (root / path).write_text(text)
    git(root, 'add', '-A')
    git(root, 'commit', '-q', '-m', message)


class Classification(unittest.TestCase):
    def classify(self, *paths):
        return impact.classify(paths, MEMBERS, BUCK)

    def test_documentation_no_build_reads_is_only_documentation(self):
        c = self.classify('README.md', 'docs/building.md', 'crates/hirsel-llvm/README.md', 'LICENSE')
        self.assertEqual(c.docs, ['LICENSE', 'README.md', 'crates/hirsel-llvm/README.md', 'docs/building.md'])
        self.assertFalse(c.needs_buck2)
        self.assertEqual(c.campaigns, set())

    def test_documentation_a_build_reads_is_an_input(self):
        # Literally (include_bytes of the grammar) or under a data glob (the corpus).
        c = self.classify('docs/grammar.md', 'conformance/M2-RUNTIME.md')
        self.assertEqual(c.docs, [])
        self.assertEqual(c.owned, ['conformance/M2-RUNTIME.md', 'docs/grammar.md'])
        self.assertTrue(c.needs_buck2)

    def test_gate_tooling_selects_its_tests(self):
        c = self.classify('tools/gate', 'tools/land', 'tools/buck2/sync.py', 'tools/tests/gate.py', 'tools/main-full-run.sh')
        self.assertEqual(len(c.tools), 5)
        self.assertFalse(c.needs_buck2)

    def test_the_native_benchmark_harness_is_tooling_and_its_programs_are_inputs(self):
        harness = ['bench/native/analysis.py', 'bench/native/run', 'bench/native/tests/test_analysis.py',
                   'bench/native/c/arena.h', 'bench/native/manifest.json']
        c = impact.classify([*harness, 'bench/native/programs/sum/main.hir'], MEMBERS,
                            BUCK + '\ndata = glob(["bench/native/programs/**"])')
        self.assertEqual(c.tools, sorted(harness))
        self.assertEqual(c.owned, ['bench/native/programs/sum/main.hir'])
        self.assertIn('bench/native/tests', [name for name, _ in gating.tool_tests()])

    def test_sizing_evidence_and_generator_do_not_select_every_test(self):
        c = self.classify('tools/buck2/test_sizes_from_usage.py', 'tools/buck2/test-sizes.json',
                          'tools/buck2/test_sizes.bzl')
        self.assertEqual(len(c.tools), 3)
        self.assertFalse(c.needs_buck2)

    def test_request_diff_selects_only_changed_added_or_removed_binaries(self):
        old = 'TEST_REQUESTS = {"same": [1, 512, ""], "changed": [2, 512, ""], "gone": [1, 512, ""]}'
        new = '# evidence refresh\nTEST_REQUESTS = {"changed": [1, 512, ""], "same": [1, 512, ""], "new": [1, 512, ""]}'
        self.assertEqual(impact.changed_test_requests(old, new), {'changed', 'gone', 'new'})
        self.assertEqual(impact.changed_test_requests(old, '# comment\n' + old), set())
        for text in ['TEST_REQUESTS = {}\nother = 1', 'TEST_REQUESTS = call()',
                     'TEST_REQUESTS = {"bad": [0, 512, ""]}']:
            with self.subTest(text=text), self.assertRaises(ValueError):
                impact.changed_test_requests(old, text)

    def test_build_wide_inputs_select_everything(self):
        for path in ['Cargo.toml', '.buckconfig', 'tools/buck2/defs.bzl', 'tools/buck2/test_launcher.sh',
                     'tools/buck2/prelude_overlay.py', 'third-party/BUCK', 'rust-toolchain.toml']:
            with self.subTest(path=path):
                self.assertTrue(self.classify(path).everything)
        self.assertTrue(self.classify('crates/hirsel-gone/src/lib.rs').everything)
        self.assertFalse(self.classify('Cargo.lock').everything)

    def test_a_crates_library_source_selects_its_campaigns_and_its_tests_do_not(self):
        lexer = self.classify('crates/hirsel-lexer/src/lib.rs')
        self.assertEqual(lexer.campaigns, {impact.CONFORMANCE, impact.AGREEMENT})
        self.assertEqual(lexer.crates, {'hirsel-lexer'})
        self.assertEqual(self.classify('crates/hirsel-llvm/src/emit.rs').campaigns, {impact.NATIVE_SMOKE})
        self.assertEqual(self.classify('crates/hirsel-types/Cargo.toml').campaigns, set(impact.CAMPAIGNS))
        self.assertEqual(self.classify('crates/hirsel-cli/src/main.rs').campaigns, set())
        tests = self.classify('crates/hirsel-lexer/tests/it/keywords.rs', 'crates/hirsel-llvm/benches/b.rs')
        self.assertEqual((tests.campaigns, tests.crates), (set(), set()))
        self.assertEqual(tests.owned, ['crates/hirsel-lexer/tests/it/keywords.rs', 'crates/hirsel-llvm/benches/b.rs'])

    def test_a_campaign_binary_does_not_select_other_commands_campaigns(self):
        for path, campaigns in impact.SOURCE_CAMPAIGNS.items():
            with self.subTest(path=path):
                c = impact.classify([path], {'hirsel-conformance'}, BUCK)
                self.assertEqual(c.campaigns, campaigns)
                self.assertTrue(c.needs_buck2)
        c = impact.classify(['crates/hirsel-conformance/src/lib.rs'], {'hirsel-conformance'}, BUCK)
        self.assertEqual(c.campaigns, set(impact.CAMPAIGNS))

    def test_a_crate_readme_read_by_a_test_does_not_become_library_source(self):
        c = impact.classify(['crates/hirsel-conformance/README.md'], {'hirsel-conformance'},
                            'data = glob(["crates/**"])')
        self.assertEqual(c.owned, ['crates/hirsel-conformance/README.md'])
        self.assertEqual(c.crates, set())
        self.assertEqual(c.campaigns, set())

    def test_every_workspace_crate_has_a_row_and_the_repository_passes_the_policy(self):
        self.assertEqual(set(impact.CRATE_CAMPAIGNS), gating.members(REPO))
        self.assertEqual(gating.policy_errors(REPO), [])

    def test_buck_edits_select_their_targets_and_a_header_edit_everything(self):
        changed = BUCK.replace('"conformance/**"', '"conformance/**",\n        "stdlib/**"')
        self.assertEqual(impact.buck_targets(BUCK, changed), {'hirsel-lexer-test-it'})
        added = BUCK + '\nhirsel_test(\n    name = "hirsel-llvm-test-it",\n)\n'
        self.assertEqual(impact.buck_targets(BUCK, added), {'hirsel-llvm-test-it'})
        self.assertEqual(impact.buck_targets(BUCK, BUCK), set())
        self.assertIs(impact.buck_targets(BUCK, BUCK.replace('"hirsel_test")', '"hirsel_test", "x")')), True)
        self.assertEqual(graph.Graph(QUERY_BUCK).macro_targets({'hirsel-driver-test-it'}),
                         {'hirsel-driver-test-it-harness', 'hirsel-driver-test-it-shard-1',
                          'hirsel-driver-test-it-shard-2'})

    def test_the_generated_buck_parses_into_every_rule(self):
        header, rules = impact.buck_rules((REPO / 'BUCK').read_text())
        self.assertIn('load(', header)
        self.assertIn('hirsel-driver-test-it', rules)
        self.assertEqual(len(rules), (REPO / 'BUCK').read_text().count('\n    name = '))


class Workspace(unittest.TestCase):
    """The workspace rules the selection depends on."""

    def test_main_advancing_does_not_select_sibling_changes(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            scratch_repo(root)
            commit(root, {'shared.txt': 'initial'}, 'Initial fixture')
            git(root, 'switch', '-q', '-c', 'lane')
            commit(root, {'ours.txt': 'lane'}, 'Lane fixture')
            git(root, 'switch', '-q', 'main')
            commit(root, {'upstream.txt': 'sibling'}, 'Sibling fixture')
            git(root, 'switch', '-q', 'lane')
            (root / 'shared.txt').write_text('unstaged')
            (root / 'new.txt').write_text('untracked')
            self.assertEqual(set(gating.changed_paths(gating.merge_base('main', root), root)),
                             {'ours.txt', 'shared.txt', 'new.txt'})

    def test_a_deleted_file_stands_for_one_surviving_sibling(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / 'crates/a/src').mkdir(parents=True)
            for path in ['crates/a/src/lib.rs', 'crates/a/src/kept.rs', 'crates/a/Cargo.toml']:
                (root / path).write_text('')
            self.assertEqual(
                gating.owned_paths(['crates/a/src/gone.rs', 'crates/a/Cargo.toml'], root),
                ['crates/a/Cargo.toml', 'crates/a/src/kept.rs'])
            self.assertEqual(gating.owned_paths(['crates/b/src/gone/deep.rs'], root), [])

    def test_a_deleted_corpus_file_in_a_large_directory_keeps_the_query_small(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            corpus = root / 'conformance/MODE'
            corpus.mkdir(parents=True)
            for index in range(2000):
                (corpus / f'MODE-2-case-{index:04}-ok.hir').write_text('')
            files = gating.owned_paths(['conformance/MODE/MODE-2-renamed-bad.hir'], root)
            self.assertEqual(files, ['conformance/MODE/MODE-2-case-0000-ok.hir'])

    def test_a_query_reaches_buck2_through_an_argfile(self):
        # One argument above Linux's 128 KiB limit per string would fail exec.
        expression = ' + '.join(f'owner("conformance/MODE/case-{i:05}.hir")' for i in range(4000))
        self.assertGreater(len(expression), 128 * 1024)
        seen = []

        def buck2(command, *arguments, **kwargs):
            seen.append((command, arguments, Path(arguments[0][1:]).read_text()))
            return subprocess.CompletedProcess([], 0, stdout='root//:a\n', stderr='')
        with patch.object(gating, 'buck2', buck2):
            self.assertEqual(gating.uquery(expression), ['root//:a'])
        [(command, arguments, text)] = seen
        self.assertEqual(command, 'uquery')
        self.assertEqual(len(arguments), 1)
        self.assertTrue(arguments[0].startswith('@'))
        self.assertEqual(text, expression + '\n')

    def test_only_tool_crates_may_depend_on_the_driver(self):
        def package(name, *dependencies):
            return dict(id=name, name=name, dependencies=[
                dict(name=dependency, kind=kind) for dependency, kind in dependencies])
        metadata = {
            'workspace_members': ['hirsel-driver', 'hirsel-cli', 'hirsel-pipeline-tests',
                                  'hirsel-rc', 'hirsel-lir', 'hirsel-types', 'hirsel-interp'],
            'packages': [
                package('hirsel-driver', ('hirsel-rc', None), ('hirsel-engine', None)),
                package('hirsel-cli', ('hirsel-driver', None), ('hirsel-engine', None)),
                package('hirsel-interp', ('hirsel-engine', 'dev')),
                package('hirsel-pipeline-tests', ('hirsel-driver', 'dev')),
                package('hirsel-rc', ('hirsel-lir', None), ('hirsel-driver', 'dev')),
                package('hirsel-lir', ('hirsel-driver', None)),
                package('hirsel-types', ('serde', 'dev')),
                package('external', ('hirsel-driver', 'dev')),
            ],
        }
        self.assertEqual(gating.driver_dependents(metadata), ['hirsel-interp', 'hirsel-lir', 'hirsel-rc'])
        # The control: without those edges nothing is refused.
        for name in ['hirsel-rc', 'hirsel-lir', 'hirsel-interp']:
            next(p for p in metadata['packages'] if p['name'] == name)['dependencies'] = []
        self.assertEqual(gating.driver_dependents(metadata), [])

    def test_manifests_give_every_declared_dependency_edge(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / 'Cargo.toml').write_text('[workspace]\nmembers = ["crates/*"]\n')
            for name, text in [('a', '[dependencies]\nb = { path = "../b" }\n'
                                     '[dev-dependencies]\nd = { package = "hirsel-driver", path = "../d" }\n'),
                               ('b', '[target.\'cfg(unix)\'.build-dependencies]\nc = "1"\n')]:
                (root / 'crates' / name).mkdir(parents=True)
                (root / 'crates' / name / 'Cargo.toml').write_text(f'[package]\nname = "{name}"\n' + text)
            metadata = gating.workspace_metadata(root)
            self.assertEqual(metadata['workspace_members'], ['a', 'b'])
            self.assertEqual([p['dependencies'] for p in metadata['packages']], [
                [dict(name='b', kind=None), dict(name='hirsel-driver', kind='dev')],
                [dict(name='c', kind='build')],
            ])
            self.assertEqual(gating.driver_dependents(metadata), ['a'])

    def test_a_crate_with_one_test_binary_rejects_new_top_level_test_files(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for path, text in [('crates/merged/tests/it/main.rs', 'mod old;\n'),
                               ('crates/merged/tests/it/old.rs', ''),
                               ('crates/merged/tests/new.rs', '#[test]\nfn t() {}\n'),
                               ('crates/merged/tests/allocator.rs',
                                '// own test binary: installs a global allocator.\n'),
                               ('crates/single/tests/one.rs', ''),
                               ('crates/single/tests/two.rs', '')]:
                (root / path).parent.mkdir(parents=True, exist_ok=True)
                (root / path).write_text(text)
            self.assertEqual(gating.stray_test_binaries(root), ['crates/merged/tests/new.rs'])

    def test_full_gate_skips_reach_their_test_targets(self):
        import sync
        buck = (REPO / 'BUCK').read_text()
        _, rules = impact.buck_rules(buck)
        for binary, cases in sync.FULL_GATE_SKIPS.items():
            self.assertIn(f'    skips = {json.dumps(cases)},\n', rules[binary])


# A root BUCK for selections over the real repository: a library, its tests
# and a binary's test that reaches it through the driver.
QUERY_BUCK = '''# @generated
hirsel_library(
    name = "hirsel-check",
    srcs = glob(["crates/hirsel-check/src/**"]),
    crate_root = "crates/hirsel-check/src/lib.rs",
)
hirsel_library(
    name = "hirsel-checker",
    srcs = glob(["crates/hirsel-checker/src/**"]),
)
hirsel_test(
    name = "hirsel-check-unittest",
    srcs = glob(["crates/hirsel-check/src/**"]),
    deps = ["//:hirsel-check"],
)
hirsel_test(
    name = "hirsel-checker-unittest",
    deps = ["//:hirsel-checker"],
)
hirsel_library(
    name = "hirsel-driver",
    srcs = glob(["crates/hirsel-driver/src/**"]),
    deps = ["//:hirsel-check"],
)
hirsel_binary(
    name = "hirsel-cli-bin-hirsel",
    srcs = glob(["crates/hirsel-cli/src/**"]),
    deps = ["//:hirsel-driver"],
)
hirsel_test(
    name = "hirsel-cli-test-cli",
    bin_exes = {"hirsel": "//:hirsel-cli-bin-hirsel"},
)
hirsel_test(
    name = "hirsel-cli-test-examples",
    shards = 2,
    bin_exes = {"hirsel": "//:hirsel-cli-bin-hirsel"},
    data = glob(["examples/**"]),
)
hirsel_library(
    name = "hirsel-lexer",
    srcs = glob(["crates/hirsel-lexer/src/**"]),
    doctests = True,
)
hirsel_test(
    name = "hirsel-lexer-test-it",
    srcs = glob(["crates/hirsel-lexer/tests/**"]),
    data = ["crates/hirsel-lexer/tests/conformance.rs"],
    deps = ["//:hirsel-lexer"],
)
hirsel_test(
    name = "hirsel-driver-test-it",
    shards = 2,
    deps = ["//:hirsel-driver"],
)
hirsel_command_test(
    name = "gate-conformance-shard-1",
    binary = "//:hirsel-cli-bin-hirsel",
    data = glob(["conformance/**"]),
)
'''


class Graph(unittest.TestCase):
    """graph.py's static reading of the root BUCK."""

    def test_the_repositorys_buck_expands_every_macro(self):
        found = graph.Graph((REPO / 'BUCK').read_text(), REPO)
        tests = found.all_tests()
        self.assertIn('hirsel-driver-test-it-shard-1', tests)
        self.assertNotIn('hirsel-driver-test-it', tests)
        self.assertFalse(any(name.startswith('gate-') for name in tests))
        self.assertTrue(found.owners(['crates/hirsel-lexer/src/lib.rs']) >= {'hirsel-lexer'})

    def test_globs_follow_buck2(self):
        glob = graph.Glob(('crates/**/src/*.rs', 'docs/**'), ('docs/spec/**',))
        self.assertTrue(glob.matches('crates/a/b/src/lib.rs'))
        self.assertTrue(glob.matches('crates/src/lib.rs'))
        self.assertFalse(glob.matches('crates/a/src/x/lib.rs'))
        self.assertTrue(glob.matches('docs/tour.md'))
        self.assertFalse(glob.matches('docs/spec/rules.md'))
        # A subpackage's files belong to it, not to the root's globs.
        self.assertFalse(graph.Glob(('**',)).matches('tools/buck2/defs.bzl'))

    def test_a_test_reaches_what_its_harness_and_binaries_depend_on(self):
        found = graph.Graph(QUERY_BUCK)
        self.assertEqual(found.tests(found.rdeps({'hirsel-driver'})),
                         {'hirsel-cli-test-cli', 'hirsel-cli-test-examples-shard-1', 'hirsel-cli-test-examples-shard-2',
                          'hirsel-driver-test-it-shard-1', 'hirsel-driver-test-it-shard-2'})
        self.assertEqual(found.crate_tests({'hirsel-lexer'}), {'hirsel-lexer', 'hirsel-lexer-test-it'})
        with self.assertRaisesRegex(ValueError, 'unknown macro'):
            graph.Graph('hirsel_new_rule(name = "x")\n')


class Queries(unittest.TestCase):
    """The floor, the suggestion and a lane's choice, over graph.py's static BUCK."""

    @staticmethod
    def selection(paths, root=None, buck=False):
        selection = gating.Selection.__new__(gating.Selection)
        selection.base = 'HEAD'
        selection.root = root or REPO
        selection.paths = paths
        selection.graph = graph.Graph(QUERY_BUCK, selection.root)
        selection.classified = impact.classify(paths, MEMBERS | {'hirsel-check', 'hirsel-driver'}, BUCK)
        if buck:
            selection.classified.buck = buck
        return selection

    @staticmethod
    def run_query(method, selection):
        """The plan, and the Buck2 queries it asked (it must ask none)."""
        asked = []

        def uquery(expression):
            asked.append(expression)
            return []
        with patch.object(gating, 'uquery', uquery):
            plan = method(selection)
        return plan, asked

    def test_a_crates_source_floors_its_own_tests_and_suggests_its_campaigns(self):
        selection = self.selection(['crates/hirsel-check/src/lib.rs'])
        floor, asked = self.run_query(gating.Selection.floor, selection)
        self.assertEqual(asked, [])
        # A crate's tests, not a longer-named crate's.
        self.assertEqual(floor.tests, {'root//:hirsel-check-unittest'})
        self.assertEqual((floor.campaigns, floor.conformance), (set(), set()))
        plan, _ = self.run_query(gating.Selection.suggest, selection)
        self.assertEqual(plan.campaigns, {impact.CONFORMANCE, impact.AGREEMENT})
        # Every test reading it through a dependency, but not the examples.
        self.assertEqual(plan.tests, {'root//:hirsel-check-unittest', 'root//:hirsel-cli-test-cli',
                                      'root//:hirsel-driver-test-it-shard-1', 'root//:hirsel-driver-test-it-shard-2'})
        expected = [f'root//:gate-{name}-shard-{index}'
                    for name, count in [('conformance', gating.CONFORMANCE_SHARDS),
                                        ('agree', gating.AGREE_SHARDS)]
                    for index in range(1, count + 1)]
        self.assertCountEqual(plan.campaign_targets(), expected)

    def test_changed_test_code_floors_only_the_tests_that_own_it(self):
        selection = self.selection(['crates/hirsel-lexer/tests/conformance.rs'])
        floor, asked = self.run_query(gating.Selection.floor, selection)
        self.assertEqual(asked, [])
        self.assertEqual(floor.tests, {'root//:hirsel-lexer-test-it'})

    def test_a_build_wide_edit_suggests_every_test_and_campaign(self):
        plan, asked = self.run_query(gating.Selection.suggest, self.selection(['.buckconfig']))
        self.assertEqual(asked, [])
        self.assertEqual(plan.tests, {f'root//:{name}' for name in graph.Graph(QUERY_BUCK).all_tests()})
        self.assertIn('root//:hirsel-lexer', plan.tests)  # a library with doctests
        self.assertNotIn('root//:gate-conformance-shard-1', plan.tests)
        self.assertEqual(plan.campaigns, set(impact.CAMPAIGNS))

    def test_resizing_suggests_runs_without_rdeps_or_unrelated_campaigns(self):
        selection = self.selection([])
        selection.classified = impact.Classified(test_requests={
            'hirsel-cli-test-cli', 'gate-conformance', 'gate-native-full'})
        plan, _ = self.run_query(gating.Selection.suggest, selection)
        self.assertEqual(plan.tests, {'root//:hirsel-cli-test-cli'})
        self.assertEqual(plan.campaigns, {impact.CONFORMANCE})

    def test_the_examples_run_only_for_the_cli_the_driver_or_an_example(self):
        examples = {'root//:hirsel-cli-test-examples-shard-1', 'root//:hirsel-cli-test-examples-shard-2'}
        plan, _ = self.run_query(gating.Selection.suggest, self.selection(['crates/hirsel-check/src/lib.rs']))
        self.assertEqual(plan.tests & examples, set())
        self.assertIn('root//:hirsel-cli-test-cli', plan.tests)
        for path in ['crates/hirsel-driver/src/lib.rs', 'crates/hirsel-cli/src/main.rs']:
            with self.subTest(path=path):
                plan, _ = self.run_query(gating.Selection.suggest, self.selection([path]))
                self.assertTrue(examples <= plan.tests)

    def test_a_lane_names_targets_campaigns_corpus_cases_and_programs(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for case in ['conformance/MODE/MODE-1-a-ok.hir', 'conformance/MODE/MODE-1-a-bad.hir', 'conformance/SEM/x-run.hir']:
                (root / case).parent.mkdir(parents=True, exist_ok=True)
                (root / case).write_text('// expect: run\n')
            selection = self.selection([], root=root)
            plan, _ = self.run_query(lambda s: s.choose([
                '//:hirsel-driver-test-it', 'hirsel-check-unittest', 'agreement', 'conformance:conformance/MODE/',
                'agreement:conformance/SEM/x-run.hir', 'native:conformance/SEM/x-run.hir']), selection)
            self.assertEqual(plan.tests, {'root//:hirsel-driver-test-it-shard-1', 'root//:hirsel-driver-test-it-shard-2',
                                          'root//:hirsel-check-unittest'})
            self.assertEqual(plan.campaigns, {impact.AGREEMENT})
            self.assertEqual(plan.conformance, {'conformance/MODE/MODE-1-a-ok.hir', 'conformance/MODE/MODE-1-a-bad.hir'})
            self.assertEqual(plan.native, {'conformance/SEM/x-run.hir'})
            for token in ['hirsel-nothing', 'gate-conformance-shard-1', 'conformance:conformance/NONE',
                          'native:conformance/none.hir']:
                with self.subTest(token=token), self.assertRaises(SystemExit):
                    self.run_query(lambda s: s.choose([token]), selection)

    def test_generated_program_ids_select_both_native_backends(self):
        kinds = ['success', 'overflow', 'underflow', 'dividebyzero', 'index', 'requires']
        programs = {f'generated/{kind}-{seed}' for kind in kinds for seed in [0, 2**64 - 1]}
        selection = self.selection([])
        plan, asked = self.run_query(lambda s: s.choose(['native:' + p for p in sorted(programs)]), selection)
        self.assertEqual(asked, [])
        self.assertEqual(plan.native, programs)
        self.assertEqual(plan.campaign_targets(), gating.NATIVE_TARGETS)
        self.assertEqual(plan.config(), ['-c', 'hirsel.native_filters=' + '|'.join(sorted(programs))])
        for program in ['generated/unknown-0', 'generated/success--1', 'generated/success-01',
                        'generated/success-18446744073709551616', 'generated/success-1/child',
                        'generated/success-1.0', 'generated/success-١', 'generated/success-' + '9' * 5000]:
            with self.subTest(program=program), self.assertRaises(SystemExit):
                selection.choose(['native:' + program])

    def test_a_plan_settles_slices_and_names_its_targets(self):
        import sync
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            cases = [f'conformance/SEM/case-{i:04}-run.hir' for i in range(sync.CONFORMANCE_SLICE_PROGRAMS + 1)]
            for case in cases + [f'conformance/OTHER/case-{i:04}-run.hir' for i in range(len(cases))]:
                (root / case).parent.mkdir(parents=True, exist_ok=True)
                (root / case).write_text('// rule: SEM-1\n// expect: run\n')
            (root / 'conformance/SEM/rejected-bad.hir').write_text('// rule: SEM-1\n// expect: reject SEM-1 at line 3\n')
            plan = gating.Plan(conformance=set(cases[:sync.CONFORMANCE_SLICE_PROGRAMS + 1]),
                               agreement={cases[0], 'conformance/SEM/rejected-bad.hir'})
            plan.settle(root)
            # Agreement mutates no case expected to reject.
            self.assertEqual(plan.agreement, {cases[0]})
            config = plan.config()
            self.assertEqual(config[0], '-c')
            conformance = config[1].removeprefix('hirsel.conformance_paths=').split(';')
            self.assertEqual(len(conformance), 2)
            self.assertEqual(sorted(p for part in conformance for p in part.split('|')), sorted(cases))
            self.assertEqual(config[3], f'hirsel.agree_paths={cases[0]}')
            self.assertEqual([gating.short(t) for t in plan.campaign_targets()], [
                'gate-conformance-selected-shard-1', 'gate-conformance-selected-shard-2', 'gate-agree-selected-shard-1'])
            # Cases spread over up to 16 slices of at least the smallest slice's cases.
            self.assertEqual([len(part) for part in gating.slices(list(range(1000)), 20)], [63] * 8 + [62] * 8)
            self.assertEqual(gating.slices(list(range(5)), 20), [[0, 1, 2, 3, 4]])
            # More than half the corpus is the whole campaign; a whole campaign absorbs its cases.
            whole = gating.Plan(conformance=set(cases) | {'conformance/OTHER/case-0000-run.hir'},
                                agreement={cases[0]}, campaigns={impact.AGREEMENT})
            whole.settle(root)
            self.assertEqual((whole.conformance, whole.agreement), (set(), set()))
            self.assertEqual(whole.campaigns, {impact.CONFORMANCE, impact.AGREEMENT})


class Corpus(unittest.TestCase):
    """Which corpus cases a change selects: twins, the ratchet, std's import closure and packages."""

    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name).resolve()

    def write(self, files):
        for path, text in files.items():
            (self.root / path).parent.mkdir(parents=True, exist_ok=True)
            (self.root / path).write_text(text)

    def sidecar(self, case, dependencies):
        depth = '../' * case.count('/')
        return json.dumps({'schema': 1, 'package': {'name': 'standalone', 'version': '0'},
                           'sources': [{'path': case.rsplit('/', 1)[1], 'module': []}],
                           'dependencies': [dict(manifest=depth + manifest, package=dict(name=name, version='1'),
                                                 expose=expose) for manifest, name, expose in dependencies]})

    def test_a_changed_case_selects_its_twins_and_a_deleted_one_its_survivors(self):
        self.write({'conformance/MODE/MODE-1-x-ok.hir': '', 'conformance/MODE/MODE-1-x-bad.hir': '',
                    'conformance/MODE/MODE-1-x-run.hir': '', 'conformance/MODE/MODE-1-xy-ok.hir': '',
                    'conformance/MODE/MODE-1-plain.hir': ''})
        self.assertEqual(impact.changed_programs(['conformance/MODE/MODE-1-x-bad.hir.json'], self.root), [
            'conformance/MODE/MODE-1-x-bad.hir', 'conformance/MODE/MODE-1-x-ok.hir', 'conformance/MODE/MODE-1-x-run.hir'])
        self.assertEqual(impact.changed_programs(['conformance/MODE/MODE-1-plain.hir'], self.root),
                         ['conformance/MODE/MODE-1-plain.hir'])
        self.assertEqual(impact.changed_programs(['conformance/MODE/MODE-1-xy-bad.hir'], self.root),
                         ['conformance/MODE/MODE-1-xy-ok.hir'])
        self.assertEqual(impact.changed_programs(['conformance/README.md', 'stdlib/std@1/a.hir'], self.root), [])

    def test_a_ratchet_edit_selects_the_cases_its_lines_name(self):
        self.write({'conformance/SEM/a-run.hir': '', 'conformance/SEM/b-run.hir': ''})
        diff = ('--- a/conformance/passing.txt\n+++ b/conformance/passing.txt\n@@ -1 +1 @@\n'
                '-conformance/SEM/a-run.hir\n+conformance/SEM/b-run.hir\n+# a comment\n')
        self.assertEqual(impact.ratchet_programs(diff), {'conformance/SEM/a-run.hir', 'conformance/SEM/b-run.hir'})
        self.assertEqual(impact.changed_programs([impact.PASSING], self.root, impact.ratchet_programs(diff)),
                         ['conformance/SEM/a-run.hir', 'conformance/SEM/b-run.hir'])

    def std(self):
        std = {'schema': 1, 'package': {'name': 'std', 'version': '1'}, 'dependencies': [], 'sources': [
            {'path': 'yaml.hir', 'module': ['yaml']}, {'path': 'toml.hir', 'module': ['toml']},
            {'path': 'format.hir', 'module': ['internal', 'format']}, {'path': 'list.hir', 'module': ['list']},
            {'path': 'identity.hir', 'module': ['probe']}]}
        self.write({impact.STD_MANIFEST: json.dumps(std),
                    'stdlib/std@1/yaml.hir': 'pub fn parse(s: String) -> u64 { internal.format.pad(s) }\n',
                    'stdlib/std@1/toml.hir': 'pub fn parse(s: String) -> u64 { 0u64 }\n',
                    'stdlib/std@1/format.hir': 'pub fn pad(s: String) -> u64 { 1u64 }\n',
                    'stdlib/std@1/list.hir': 'pub fn sort[T](mut xs: List[T]) -> () { () }\n',
                    'stdlib/std@1/identity.hir': 'pub fn identity(x: u64) -> u64 { x }\n',
                    impact.CORE_SIGNATURES: 'fn list.push[T](mut xs: List[T], take x: T) -> ()\n'})
        std_dep = (impact.STD_MANIFEST, 'std', [])
        cases = {
            'conformance/SEM/yaml-run.hir': ('fn main() -> u64 { yaml.parse("a") }', [std_dep]),
            'conformance/SEM/toml-run.hir': ('fn main() -> u64 { toml . parse("a") }', [std_dep]),
            'conformance/SEM/push-run.hir': ('fn main(mut xs: List[u64]) -> () { list.push(mut xs, 1u64) }', [std_dep]),
            'conformance/SEM/sort-run.hir': ('fn main(mut xs: List[u64]) -> () { list.sort(mut xs) }', [std_dep]),
            'conformance/SEM/alias-run.hir': ('fn main() -> u64 { std.identity(1u64) }',
                                              [(impact.STD_MANIFEST, 'std', [{'head': 'std', 'module': ['probe']}])]),
            'conformance/SEM/no-std-run.hir': ('fn main() -> u64 { yaml.parse("a") }', []),
            'conformance/SEM/standalone-run.hir': ('fn main() -> u64 { yaml.parse("a") }', None),
        }
        for case, (source, dependencies) in cases.items():
            self.write({case: source + '\n'})
            if dependencies is not None:
                self.write({case + '.json': self.sidecar(case, dependencies)})
        return impact.Std(self.root)

    def dependents(self, *modules):
        std = self.std()
        return [p.rsplit('/', 1)[1] for p in impact.std_dependents(
            None if modules == (None,) else set(modules), self.root, std)]

    def test_a_std_module_selects_the_cases_whose_import_closure_includes_it(self):
        self.assertEqual(self.dependents(('yaml',)), ['yaml-run.hir'])
        self.assertEqual(self.dependents(('toml',)), ['toml-run.hir'])
        # Through another std module, and through an exposed alias.
        self.assertEqual(self.dependents(('internal', 'format')), ['yaml-run.hir'])
        self.assertEqual(self.dependents(('probe',)), ['alias-run.hir'])
        # core and std share the head `list`: `list.push` is core's, `list.sort` std's (LANG-4).
        self.assertEqual(self.dependents(('list',)), ['sort-run.hir'])
        # Only a sidecar selects std, so the cases without std select nothing.
        self.assertEqual(self.dependents(None), ['alias-run.hir', 'sort-run.hir', 'toml-run.hir', 'yaml-run.hir'])

    def test_a_std_file_maps_to_its_module_and_any_other_std_file_to_all_of_std(self):
        std = self.std()
        self.assertEqual(impact.changed_std_modules(['stdlib/std@1/format.hir', 'README.md'], std), {('internal', 'format')})
        self.assertIsNone(impact.changed_std_modules([impact.STD_MANIFEST], std))
        self.assertIsNone(impact.changed_std_modules(['stdlib/std@1/new.hir'], std))
        self.assertEqual(impact.changed_std_modules(['stdlib/unicode16/x.txt'], std), set())

    def manifest_change(self, manifest, files=None):
        scratch_repo(self.root)
        commit(self.root, {}, 'Base std')
        files = {impact.STD_MANIFEST: json.dumps(manifest, indent=2, sort_keys=True), **(files or {})}
        for path, text in files.items():
            if text is None:
                (self.root / path).unlink()
            else:
                self.write({path: text})
        return Queries.selection(list(files), root=self.root)

    def assert_std_floor(self, selection, previous, modules, cases):
        self.assertEqual(impact.changed_std_modules(selection.paths, impact.Std(self.root), previous), modules)
        floor, asked = Queries.run_query(gating.Selection.floor, selection)
        self.assertEqual(asked, [])
        self.assertEqual(floor.conformance, {f'conformance/SEM/{case}-run.hir' for case in cases})
        self.assertEqual(floor.campaigns, set())
        with patch.object(gating, 'owned_paths', lambda paths, root=None: list(paths)):
            plan, _ = Queries.run_query(gating.Selection.suggest, selection)
        self.assertEqual(plan.agreement, floor.conformance)
        self.assertEqual(plan.campaigns, set())

    def test_a_manifest_addition_selects_only_the_new_modules_cases(self):
        previous = self.std()
        manifest = json.loads((self.root / impact.STD_MANIFEST).read_text())
        manifest['sources'].append({'path': 'csv.hir', 'module': ['csv']})
        case = 'conformance/SEM/csv-run.hir'
        selection = self.manifest_change(manifest, {
            'stdlib/std@1/csv.hir': 'pub fn parse(s: String) -> u64 { 0u64 }\n',
            case: '// expect: run 0\nfn main() -> u64 { csv.parse("a") }\n',
            case + '.json': self.sidecar(case, [(impact.STD_MANIFEST, 'std', [])]),
        })
        self.assert_std_floor(selection, previous, {('csv',)}, ['csv'])

    def test_a_manifest_removal_retains_the_old_transitive_import_closure(self):
        previous = self.std()
        manifest = json.loads((self.root / impact.STD_MANIFEST).read_text())
        manifest['sources'] = [source for source in manifest['sources'] if source['path'] != 'format.hir']
        selection = self.manifest_change(manifest, {'stdlib/std@1/format.hir': None})
        self.assert_std_floor(selection, previous, {('internal', 'format')}, ['yaml'])

    def test_a_manifest_removal_distinguishes_a_removed_std_member_from_core(self):
        previous = self.std()
        manifest = json.loads((self.root / impact.STD_MANIFEST).read_text())
        manifest['sources'] = [source for source in manifest['sources'] if source['path'] != 'list.hir']
        selection = self.manifest_change(manifest, {'stdlib/std@1/list.hir': None})
        self.assert_std_floor(selection, previous, {('list',)}, ['sort'])

    def test_a_manifest_removal_of_one_source_still_changes_its_shared_module(self):
        self.std()
        manifest = json.loads((self.root / impact.STD_MANIFEST).read_text())
        manifest['sources'].append({'path': 'yaml_extra.hir', 'module': ['yaml']})
        self.write({impact.STD_MANIFEST: json.dumps(manifest),
                    'stdlib/std@1/yaml_extra.hir': 'pub fn extra() -> u64 { 0u64 }\n'})
        previous = impact.Std(self.root)
        manifest['sources'].pop()
        selection = self.manifest_change(manifest, {'stdlib/std@1/yaml_extra.hir': None})
        self.assert_std_floor(selection, previous, {('yaml',)}, ['yaml'])

    def test_a_manifest_removal_keeps_cases_and_examples_using_an_exposed_alias(self):
        previous = self.std()
        example = 'examples/probe/main.hir'
        self.write({example: 'fn main() -> u64 { std.identity(1u64) }\n',
                    'examples/probe/hirsel.json': self.sidecar(example, [
                        (impact.STD_MANIFEST, 'std', [{'head': 'std', 'module': ['probe']}])])})
        manifest = json.loads((self.root / impact.STD_MANIFEST).read_text())
        manifest['sources'] = [source for source in manifest['sources'] if source['path'] != 'identity.hir']
        selection = self.manifest_change(manifest, {'stdlib/std@1/identity.hir': None})
        self.assert_std_floor(selection, previous, {('probe',)}, ['alias'])
        self.assertEqual(selection.std_examples(), ['probe'])

    def test_a_manifest_repoint_selects_both_old_and_new_modules(self):
        previous = self.std()
        manifest = json.loads((self.root / impact.STD_MANIFEST).read_text())
        manifest['sources'][0]['module'] = ['toml']
        selection = self.manifest_change(manifest)
        self.assert_std_floor(selection, previous, {('yaml',), ('toml',)}, ['yaml', 'toml'])

    def test_a_manifest_source_path_repoint_selects_its_module(self):
        previous = self.std()
        manifest = json.loads((self.root / impact.STD_MANIFEST).read_text())
        manifest['sources'][1]['path'] = 'toml_new.hir'
        selection = self.manifest_change(manifest, {
            'stdlib/std@1/toml_new.hir': 'pub fn parse(s: String) -> u64 { 1u64 }\n',
            'stdlib/std@1/toml.hir': None,
        })
        self.assert_std_floor(selection, previous, {('toml',)}, ['toml'])

    def test_other_manifest_fields_still_change_all_of_std(self):
        previous = self.std()
        original = json.loads((self.root / impact.STD_MANIFEST).read_text())
        scratch_repo(self.root)
        commit(self.root, {}, 'Base std')
        for field, value in [('schema', 2), ('package', {'name': 'std', 'version': '2'}),
                             ('dependencies', [{'manifest': '../other/hirsel.json'}]), ('new_field', True)]:
            with self.subTest(field=field):
                self.write({impact.STD_MANIFEST: json.dumps({**original, field: value})})
                selection = Queries.selection([impact.STD_MANIFEST], root=self.root)
                self.assert_std_floor(selection, previous, None, ['alias', 'sort', 'toml', 'yaml'])

    def test_manifest_formatting_and_source_order_do_not_change_modules(self):
        previous = self.std()
        manifest = json.loads((self.root / impact.STD_MANIFEST).read_text())
        manifest['sources'].reverse()
        selection = self.manifest_change(manifest)
        self.assert_std_floor(selection, previous, set(), [])

    def test_a_std_only_change_floors_its_closure_and_suggests_its_sample_but_no_whole_campaign(self):
        self.std()
        selection = Queries.selection(['stdlib/std@1/toml.hir'], root=self.root)
        floor, asked = Queries.run_query(gating.Selection.floor, selection)
        self.assertEqual(asked, [])
        self.assertEqual(floor.conformance, {'conformance/SEM/toml-run.hir'})
        with patch.object(gating, 'owned_paths', lambda paths, root=None: list(paths)):
            plan, _ = Queries.run_query(gating.Selection.suggest, selection)
        self.assertEqual(plan.campaigns, set())
        self.assertEqual(plan.conformance, {'conformance/SEM/toml-run.hir'})
        self.assertEqual(plan.agreement, {'conformance/SEM/toml-run.hir'})
        plan.settle(self.root)
        self.assertEqual(plan.slot_pool(), gating.EXPRESS_POOL)

    def test_a_package_fixture_selects_the_cases_whose_packages_reach_it(self):
        self.write({'packages/fixtures/vendor@1/hirsel.json': json.dumps(
                        {'schema': 1, 'package': {'name': 'vendor', 'version': '1'}, 'dependencies': [],
                         'sources': [{'path': 'api.hir', 'module': ['api']}]}),
                    'packages/fixtures/vendor@1/api.hir': 'pub fn value() -> u64 { 1u64 }\n',
                    'conformance/PROG/vendor-run.hir': 'fn main() -> u64 { api.value() }\n',
                    'conformance/PROG/other-run.hir': 'fn main() -> u64 { 1u64 }\n'})
        self.write({'conformance/PROG/vendor-run.hir.json': self.sidecar(
            'conformance/PROG/vendor-run.hir', [('packages/fixtures/vendor@1/hirsel.json', 'vendor', [])])})
        for path in ['packages/fixtures/vendor@1/api.hir', 'packages/fixtures/vendor@1/new.hir']:
            with self.subTest(path=path):
                self.assertEqual(impact.package_dependents([path], self.root), ['conformance/PROG/vendor-run.hir'])
        self.assertEqual(impact.package_dependents(['packages/fixtures/other@1/api.hir'], self.root), [])

    def test_data_every_case_reads_selects_its_whole_campaign(self):
        self.assertEqual(impact.campaign_data('docs/spec/language.md'), impact.FRONT)
        self.assertEqual(impact.campaign_data('conformance/m2-runtime.txt'), {impact.CONFORMANCE})
        self.assertEqual(impact.campaign_data('native-tests/regressions/smoke.txt'), {impact.NATIVE_SMOKE})
        for path in ['conformance/SEM/a-run.hir', impact.PASSING, 'conformance/README.md', 'stdlib/std@1/a.hir']:
            with self.subTest(path=path):
                self.assertEqual(impact.campaign_data(path), frozenset())


class CampaignTable(unittest.TestCase):
    def test_every_crate_a_campaign_runs_through_selects_it(self):
        metadata = gating.workspace_metadata(REPO)
        self.assertEqual(gating.campaign_table_errors(metadata), [])
        # The control: a row dropping a campaign its crate's dependents run through fails.
        for crate, campaign in [('hirsel-modes', impact.AGREEMENT), ('hirsel-rt', impact.NATIVE_SMOKE),
                                ('hirsel-fmt', impact.CONFORMANCE), ('hirsel-codec', impact.AGREEMENT)]:
            with self.subTest(crate=crate):
                table = dict(impact.CRATE_CAMPAIGNS)
                table[crate] = table[crate] - {campaign}
                errors = gating.campaign_table_errors(metadata, table)
                self.assertEqual(len(errors), 1, errors)
                self.assertTrue(errors[0].startswith(f'{crate} is in the closure of'), errors)
                self.assertTrue(errors[0].endswith(f'its row needs {campaign}'), errors)


class NativePrograms(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        (self.root / 'native-tests/regressions').mkdir(parents=True)
        (self.root / 'native-tests/regressions/smoke.txt').write_text('# comment\nsmoke-a.hir\nsmoke-b.hir\n')
        (self.root / 'conformance/MEM').mkdir(parents=True)
        (self.root / 'conformance/MEM/soa.hir').write_text('// expect: run\n')
        (self.root / 'conformance/MEM/rejected.hir').write_text('// expect: reject MEM-7 at line 3\n')

    def test_each_native_producer_selects_the_same_changed_program(self):
        for crate in impact.NATIVE_PARITY_CRATES:
            self.assertEqual(impact.native_programs(
                ['conformance/MEM/soa.hir', 'conformance/MEM/soa.hir.json',
                 'conformance/MEM/rejected.hir', 'conformance/MEM/deleted.hir'],
                {crate}, self.root), ['conformance/MEM/soa.hir'])

    def test_each_native_producer_selects_changed_test_suites(self):
        (self.root / 'conformance/MEM/suite.hir').write_text('// expect: test\n')
        for crate in impact.NATIVE_PARITY_CRATES:
            self.assertEqual(impact.native_programs(
                ['conformance/MEM/suite.hir', 'conformance/MEM/suite.hir.json'],
                {crate}, self.root), ['conformance/MEM/suite.hir'])

    def test_producer_without_changed_programs_uses_bounded_smoke_controls(self):
        self.assertEqual(impact.native_programs(['crates/hirsel-llvm/src/emit.rs'],
                         {'hirsel-llvm'}, self.root), ['smoke-a.hir', 'smoke-b.hir'])
        self.assertEqual(impact.native_programs([], {'hirsel-parser'}, self.root), [])

    def test_a_plan_summary_names_each_binary_once(self):
        plan = gating.Plan(tests={'root//:hirsel-cli-test-examples-shard-1', 'root//:hirsel-cli-test-examples-shard-2',
                                  'root//:hirsel-lexer-unittest'}, campaigns={impact.AGREEMENT})
        self.assertEqual(plan.summary(), ['hirsel-cli-test-examples (2 shards)', 'hirsel-lexer-unittest', 'agreement'])

    def test_chosen_native_programs_run_both_backends_with_one_filter(self):
        plan = gating.Plan(native={'conformance/MEM/soa.hir'})
        self.assertEqual(plan.campaign_targets(), ['root//:gate-native-changed', 'root//:gate-native-cranelift-changed'])
        self.assertEqual(plan.config(), ['-c', 'hirsel.native_filters=conformance/MEM/soa.hir'])


class Identity(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        scratch_repo(self.root)
        commit(self.root, {'a.txt': ''.join(f'{n}\n' for n in range(1, 11)), 'b.txt': 'b\n'}, 'Base')
        self.base = git(self.root, 'rev-parse', 'HEAD')
        git(self.root, 'switch', '-q', '-c', 'lane')
        self.lane = ''.join(f'{n}\n' for n in range(1, 11)).replace('3\n', 'three\n')
        commit(self.root, {'a.txt': self.lane}, 'Lane')

    def identity(self):
        return gating.change_identity(gating.merge_base('main', self.root), self.root)

    def test_a_clean_rebase_keeps_the_identity_even_beside_upstream_edits(self):
        before = self.identity()
        git(self.root, 'switch', '-q', 'main')
        # A sibling inserts a line above the change, edits one below it and another file.
        upstream = '0\n' + ''.join(f'{n}\n' for n in range(1, 11)).replace('7\n', 'seven\n')
        commit(self.root, {'a.txt': upstream, 'b.txt': 'B\n'}, 'Sibling')
        git(self.root, 'switch', '-q', 'lane')
        git(self.root, 'rebase', '-q', 'main')
        self.assertEqual(self.identity(), before)
        self.assertNotEqual(gating.merge_base('main', self.root), self.base)

    def test_any_edit_of_the_change_gives_a_new_identity(self):
        before = self.identity()
        (self.root / 'a.txt').write_text(self.lane.replace('three', 'three!'))
        self.assertNotEqual(self.identity(), before)
        (self.root / 'a.txt').write_text(self.lane)
        self.assertEqual(self.identity(), before)
        (self.root / 'new.txt').write_text('untracked\n')
        self.assertNotEqual(self.identity(), before)
        self.assertIn('new.txt', gating.changed_paths(self.base, self.root))

    def test_a_regenerated_api_index_keeps_the_identity_and_other_edits_do_not(self):
        before = self.identity()
        (self.root / 'docs').mkdir()
        (self.root / 'docs/api.md').write_text('1 exported definitions\n')
        self.assertEqual(self.identity(), before)
        commit(self.root, {'docs/api.md': '2 exported definitions\n'}, 'Index')
        self.assertEqual(self.identity(), before)
        (self.root / 'docs/other.md').write_text('prose\n')
        self.assertNotEqual(self.identity(), before)

    def test_no_change_has_no_identity(self):
        git(self.root, 'reset', '-q', '--hard', 'main')
        self.assertEqual(self.identity(), '')

    def test_path_digests_survive_a_clean_rebase_and_name_the_edited_path(self):
        before = gating.path_digests(gating.merge_base('main', self.root), self.root)
        self.assertEqual(list(before), ['a.txt'])
        git(self.root, 'switch', '-q', 'main')
        commit(self.root, {'a.txt': '0\n' + ''.join(f'{n}\n' for n in range(1, 11)), 'b.txt': 'B\n'}, 'Sibling')
        git(self.root, 'switch', '-q', 'lane')
        git(self.root, 'rebase', '-q', 'main')
        base = gating.merge_base('main', self.root)
        self.assertEqual(gating.path_digests(base, self.root), before)
        (self.root / 'b.txt').write_text('lane\n')
        after = gating.path_digests(base, self.root)
        self.assertEqual(gating.changed_since({'digests': before}, after), ['b.txt'])
        (self.root / 'a.txt').write_text(self.lane.replace('three', 'THREE'))
        self.assertEqual(gating.changed_since({'digests': before}, gating.path_digests(base, self.root)),
                         ['a.txt', 'b.txt'])

    def test_a_green_record_names_the_identity_and_its_crates(self):
        identity = self.identity()
        self.assertFalse(gating.is_green(identity, {'hirsel-lexer'}, self.root))
        gating.record_gate(identity, {'hirsel-lexer'}, self.root)
        self.assertTrue(gating.is_green(identity, {'hirsel-lexer'}, self.root))
        self.assertFalse(gating.is_green(identity, {'hirsel-llvm'}, self.root))
        self.assertFalse(gating.is_green('', set(), self.root))


class PoolClassification(unittest.TestCase):
    def test_three_std_cases_and_two_tests_fit_the_express_bound(self):
        cases = set(gating.corpus_cases('conformance/', REPO)[:3])
        floor = gating.Plan(conformance=cases, tests={'root//:one', 'root//:two'})
        chosen = gating.Plan(agreement=cases, tests={'root//:two'})
        plan = gating.Plan()
        plan.add(floor)
        plan.add(chosen)
        # Rejected cases are filtered by settle, so use accepted controls here.
        with patch.object(gating, 'accepted', return_value=True):
            plan.settle()
        self.assertEqual(len(plan.tests | set(plan.campaign_targets())), 4)
        self.assertEqual(plan.slot_pool(), gating.EXPRESS_POOL)
        plan.tests.add('root//:three')
        self.assertEqual(plan.slot_pool(), gating.NORMAL_POOL)

    def test_empty_small_and_sharded_selections(self):
        self.assertEqual(gating.Plan().slot_pool(), gating.EXPRESS_POOL)
        for count in (1, 4, 5, 16):
            with self.subTest(count=count):
                plan = gating.Plan(tests={f'root//:test-shard-{i}' for i in range(count)})
                self.assertEqual(plan.slot_pool(), gating.EXPRESS_POOL if count <= 4 else gating.NORMAL_POOL)

    def test_every_whole_campaign_and_filtered_native_selection_uses_normal(self):
        for campaign in impact.CAMPAIGNS:
            with self.subTest(campaign=campaign):
                self.assertEqual(gating.Plan(campaigns={campaign}).slot_pool(), gating.NORMAL_POOL)
        plan = gating.Plan(native={'conformance/TEST/TEST-1-run.hir'})
        self.assertEqual(plan.slot_pool(), gating.NORMAL_POOL)

    def test_selected_corpus_shards_count_toward_the_bound(self):
        # Four conformance slices fit, a fifth exceeds the bound.
        for count in (4, 5):
            plan = gating.Plan(conformance={f'case-{i}' for i in range(
                count * gating.CONFORMANCE_SLICE_PROGRAMS)})
            self.assertEqual(len(plan.campaign_targets()), count)
            self.assertEqual(plan.slot_pool(), gating.EXPRESS_POOL if count == 4 else gating.NORMAL_POOL)
        # settle promotes a large selection to the whole campaign before classifying.
        cases = set(gating.corpus_cases('conformance/', REPO))
        plan = gating.Plan(agreement=cases)
        plan.settle()
        self.assertIn(impact.AGREEMENT, plan.campaigns)
        self.assertEqual(plan.slot_pool(), gating.NORMAL_POOL)


class Slots(unittest.TestCase):
    def test_express_has_two_slots_even_with_six_normal_holders_and_an_older_normal_waiter(self):
        with tempfile.TemporaryDirectory() as scratch, contextlib.ExitStack() as held:
            directory = Path(scratch)
            environ = {gating.GATE_SLOTS: f'{directory}:6'}
            for _ in range(6):
                held.enter_context(gating.gate_slot(environ))
            ticket = directory / 'queue' / f'0-{os.getpid()}'
            ticket.touch()
            with patch.object(gating.time, 'sleep', side_effect=KeyboardInterrupt):
                with gating.gate_slot(environ, pool=gating.EXPRESS_POOL):
                    with gating.gate_slot(environ, pool=gating.EXPRESS_POOL):
                        with self.assertRaises(KeyboardInterrupt):
                            with gating.gate_slot(environ, pool=gating.EXPRESS_POOL):
                                self.fail('express exceeded its independent cap of two')
            self.assertEqual((directory / 'count').read_text(), '6\n')
            self.assertEqual((directory / 'express-count').read_text(), '2\n')
            self.assertTrue(ticket.exists())
            self.assertEqual(list((directory / 'express-queue').iterdir()), [])
            self.assertEqual(list(directory.glob('.express-count-*')), [])

    def test_an_older_express_waiter_does_not_block_normal(self):
        with tempfile.TemporaryDirectory() as scratch:
            directory = Path(scratch)
            queue = directory / 'express-queue'
            queue.mkdir()
            ticket = queue / f'0-{os.getpid()}'
            ticket.touch()
            environ = {gating.GATE_SLOTS: f'{directory}:1'}
            with patch.object(gating.time, 'sleep', side_effect=KeyboardInterrupt):
                with gating.gate_slot(environ):
                    with self.assertRaises(KeyboardInterrupt):
                        with gating.gate_slot(environ, pool=gating.EXPRESS_POOL):
                            self.fail('express bypassed its earlier waiter')
            self.assertEqual(list(queue.iterdir()), [ticket])

    def test_raising_the_count_admits_the_queued_head_into_the_new_slot(self):
        self.raising_count(gating.NORMAL_POOL)

    def test_raising_the_express_count_admits_its_queued_head(self):
        self.raising_count(gating.EXPRESS_POOL)

    def raising_count(self, pool):
        with tempfile.TemporaryDirectory() as scratch:
            directory = Path(scratch) / 'slots'
            environ = {gating.GATE_SLOTS: f'{directory}:1'}
            prefix = gating.POOL_PREFIX[pool]
            gating.write_gate_slot_count(directory, 1, pool=pool)
            polls = []

            class LaterWaiterBlocked(Exception):
                pass

            def later_poll(_):
                raise LaterWaiterBlocked

            def raise_count(_):
                polls.append('head waited')
                gating.write_gate_slot_count(directory, 2, pool=pool)
                # A fresh gate with a larger env cap still cannot pass the head.
                with patch.object(gating.time, 'sleep', later_poll):
                    with self.assertRaises(LaterWaiterBlocked):
                        with gating.gate_slot({gating.GATE_SLOTS: f'{directory}:6'}, pool=pool):
                            self.fail('a later gate passed the queued head')
                self.assertEqual((directory / (prefix + 'count')).read_text(), '2\n')

            with gating.gate_slot(environ, pool=pool):
                with patch.object(gating.time, 'sleep', raise_count), contextlib.redirect_stdout(io.StringIO()):
                    with gating.gate_slot(environ, pool=pool):
                        self.assertEqual(polls, ['head waited'])
                        for slot in (0, 1):
                            with open(directory / f'{prefix}slot-{slot}', 'a+') as handle:
                                with self.assertRaises(BlockingIOError):
                                    fcntl.flock(handle, fcntl.LOCK_EX | fcntl.LOCK_NB)
                self.assertEqual(list((directory / (prefix + 'queue')).iterdir()), [])

    def test_lowering_the_count_keeps_held_slots_and_excludes_retired_slots(self):
        self.lowering_count(gating.NORMAL_POOL)

    def test_lowering_the_express_count_keeps_holders_and_excludes_retired_slots(self):
        self.lowering_count(gating.EXPRESS_POOL)

    def lowering_count(self, pool):
        with tempfile.TemporaryDirectory() as scratch:
            directory = Path(scratch) / 'slots'
            environ = {gating.GATE_SLOTS: f'{directory}:2'}
            prefix = gating.POOL_PREFIX[pool]
            with contextlib.ExitStack() as first, contextlib.ExitStack() as second:
                first.enter_context(gating.gate_slot(environ, pool=pool))
                second.enter_context(gating.gate_slot(environ, pool=pool))
                gating.write_gate_slot_count(directory, 1, pool=pool)
                first.close()
                with gating.gate_slot(environ, pool=pool):
                    with open(directory / (prefix + 'slot-1'), 'a+') as handle:
                        with self.assertRaises(BlockingIOError):
                            fcntl.flock(handle, fcntl.LOCK_EX | fcntl.LOCK_NB)
                    second.close()
                    # slot-1 is now free, but the lowered cap must exclude it.
                    with patch.object(gating.time, 'sleep', side_effect=KeyboardInterrupt):
                        with self.assertRaises(KeyboardInterrupt), contextlib.redirect_stdout(io.StringIO()):
                            with gating.gate_slot(environ, pool=pool):
                                self.fail('a gate acquired a retired slot')
                self.assertEqual((directory / (prefix + 'count')).read_text(), '1\n')

    def test_a_missing_count_falls_back_to_the_host_default_not_the_environment(self):
        with tempfile.TemporaryDirectory() as scratch:
            directory = Path(scratch) / 'slots'
            # A stale `:1` from a brief must not cap the host at one slot.
            environ = {gating.GATE_SLOTS: f'{directory}:1'}
            gating.write_gate_slot_count(directory, 1)
            polls = []

            def remove_count(_):
                polls.append('waited')
                (directory / 'count').unlink()

            with gating.gate_slot(environ):
                with patch.object(gating.time, 'sleep', remove_count), contextlib.redirect_stdout(io.StringIO()):
                    with gating.gate_slot(environ):
                        self.assertEqual(polls, ['waited'])
                        self.assertFalse((directory / 'count').exists())
                        self.assertTrue((directory / 'slot-1').exists())
                        with gating.gate_slot(environ):
                            self.assertTrue((directory / 'slot-2').exists())

    def test_the_default_seeds_only_an_absent_count_and_the_environment_count_is_ignored(self):
        with tempfile.TemporaryDirectory() as scratch:
            directory = Path(scratch) / 'slots'
            with gating.gate_slot({gating.GATE_SLOTS: f'{directory}:4'}):
                self.assertEqual((directory / 'count').read_text(), '6\n')
            with gating.gate_slot({gating.GATE_SLOTS: f'{directory}:2'}, pool=gating.EXPRESS_POOL):
                self.assertEqual((directory / 'express-count').read_text(), '2\n')
            gating.write_gate_slot_count(directory, 1)
            gating.write_gate_slot_count(directory, 6, seed=True)
            with gating.gate_slot({gating.GATE_SLOTS: str(directory)}):
                self.assertEqual((directory / 'count').read_text(), '1\n')
            self.assertEqual(list(directory.glob('.count-*')), [])

    def test_unset_the_host_directory_is_used_and_none_runs_uncapped(self):
        with tempfile.TemporaryDirectory() as scratch:
            host = Path(scratch) / 'locks/gate-slots'
            with patch.object(gating, 'GATE_SLOT_DIR', host):
                self.assertIsNone(gating.gate_slot_dir({}))
                host.parent.mkdir()
                self.assertEqual(gating.gate_slot_dir({}), host)
                self.assertEqual(gating.gate_slot_dir({gating.GATE_SLOTS: f'{scratch}/other:4'}), Path(scratch) / 'other')
                self.assertIsNone(gating.gate_slot_dir({gating.GATE_SLOTS: gating.NO_SLOTS}))
                with gating.gate_slot({}):
                    self.assertEqual((host / 'count').read_text(), '6\n')

    def test_an_invalid_count_is_refused_and_the_ticket_is_removed(self):
        with tempfile.TemporaryDirectory() as scratch:
            directory = Path(scratch)
            for value in ('', '0', '-1', 'six', '1.5'):
                with self.subTest(value=value):
                    (directory / 'count').write_text(value)
                    with self.assertRaisesRegex(SystemExit, 'positive slot count'):
                        with gating.gate_slot({gating.GATE_SLOTS: f'{directory}:2'}):
                            self.fail('invalid host count was ignored')
                    self.assertEqual(list((directory / 'queue').iterdir()), [])

    def test_a_gate_waits_while_every_slot_is_held_and_takes_the_first_freed(self):
        with tempfile.TemporaryDirectory() as scratch:
            environ = {gating.GATE_SLOTS: f'{scratch}/slots'}
            gating.write_gate_slot_count(Path(scratch) / 'slots', 2)
            poll, gating.SLOT_POLL_SECONDS = gating.SLOT_POLL_SECONDS, 0.05
            self.addCleanup(setattr, gating, 'SLOT_POLL_SECONDS', poll)
            entered = threading.Event()
            with gating.gate_slot(environ), gating.gate_slot(environ):
                said = io.StringIO()

                def third():
                    with contextlib.redirect_stdout(said), gating.gate_slot(environ):
                        entered.set()

                waiter = threading.Thread(target=third)
                waiter.start()
                self.assertFalse(entered.wait(0.5))
            waiter.join(5)
            self.assertTrue(entered.is_set())
            self.assertIn('waiting for one of 2 normal gate slots', said.getvalue())
            self.assertIn('took normal gate slot', said.getvalue())

    def test_the_earlier_waiter_acquires_before_a_later_waiter_that_retries_first(self):
        self.fifo_order(gating.NORMAL_POOL)

    def test_the_express_fifo_keeps_earlier_waiters_ahead_of_later_retries(self):
        self.fifo_order(gating.EXPRESS_POOL)

    def test_the_land_fifo_keeps_earlier_waiters_ahead_of_later_retries(self):
        self.fifo_order(gating.NORMAL_POOL, land=True)

    def fifo_order(self, pool, *, land=False):
        with tempfile.TemporaryDirectory() as scratch:
            environ = {gating.GATE_SLOTS: f'{scratch}/slots:1'}
            gating.write_gate_slot_count(Path(scratch) / 'slots', 1, pool=pool)

            def slot():
                if land:
                    return gating.fifo_slot(Path(scratch) / 'slots', 1, label='land', exclusive=True)
                return gating.gate_slot(environ, pool=pool)

            early_waiting, late_waiting = threading.Event(), threading.Event()
            early_retry, late_retry = threading.Event(), threading.Event()
            late_attempted, early_entered = threading.Event(), threading.Event()
            order, errors = [], []
            late_polls = 0

            def poll(_):
                nonlocal late_polls
                if threading.current_thread().name == 'early':
                    early_waiting.set()
                    if not early_retry.wait(5):
                        raise AssertionError('earlier waiter was never released')
                else:
                    late_polls += 1
                    late_waiting.set()
                    if late_polls == 1:
                        if not late_retry.wait(5):
                            raise AssertionError('later waiter was never released')
                    else:
                        late_attempted.set()
                        if not early_entered.wait(5):
                            raise AssertionError('later waiter overtook or blocked the earlier waiter')

            def wait(name):
                try:
                    with slot():
                        order.append(name)
                        if name == 'early':
                            early_entered.set()
                        else:
                            late_attempted.set()
                except BaseException as error:
                    errors.append(error)

            early = threading.Thread(target=wait, args=('early',), name='early')
            late = threading.Thread(target=wait, args=('late',), name='late')
            with patch.object(gating.time, 'sleep', poll), contextlib.redirect_stdout(io.StringIO()):
                try:
                    with slot():
                        early.start()
                        self.assertTrue(early_waiting.wait(5))
                        late.start()
                        self.assertTrue(late_waiting.wait(5))
                    late_retry.set()
                    self.assertTrue(late_attempted.wait(5))
                    self.assertEqual(order, [])
                finally:
                    early_retry.set()
                    late_retry.set()
                    early_entered.set()
                    for waiter in (early, late):
                        if waiter.ident is not None:
                            waiter.join(5)
                self.assertFalse(early.is_alive() or late.is_alive())
                self.assertEqual(errors, [])
                self.assertEqual(order, ['early', 'late'])
                self.assertEqual(list((Path(scratch) / 'slots' / (gating.POOL_PREFIX[pool] + 'queue')).iterdir()), [])

    def test_land_reclaims_a_dead_holder_without_replacing_its_lock_file(self):
        with tempfile.TemporaryDirectory() as scratch:
            directory = Path(scratch)
            source = ('import fcntl, os, sys, time; '
                      'handle = open(sys.argv[1], "w"); '
                      'fcntl.flock(handle, fcntl.LOCK_EX); '
                      'handle.write(str(os.getpid())); handle.flush(); '
                      'print("held", flush=True); time.sleep(30)')
            holder = subprocess.Popen([sys.executable, '-c', source, str(directory / 'slot-0')],
                                      stdout=subprocess.PIPE, text=True)
            try:
                self.assertEqual(holder.stdout.readline().strip(), 'held')
                inode = (directory / 'slot-0').stat().st_ino
                queue = directory / 'queue'
                queue.mkdir()
                dead = queue / f'0-{holder.pid}'
                dead.write_text(f'{gating.process_start(holder.pid)}\n')
                holder.kill()
                holder.wait(5)
                with gating.fifo_slot(directory, 1, label='land', exclusive=True):
                    self.assertFalse(dead.exists())
                    self.assertEqual((directory / 'slot-0').stat().st_ino, inode)
            finally:
                if holder.poll() is None:
                    holder.kill()
                    holder.wait(5)
                holder.stdout.close()

    def test_land_discards_a_ticket_from_a_reused_pid_but_keeps_a_live_identity(self):
        with tempfile.TemporaryDirectory() as scratch:
            directory = Path(scratch)
            queue = directory / 'queue'
            queue.mkdir()
            stale = queue / f'0-{os.getpid()}'
            stale.write_text(f'{gating.process_start(os.getpid()) + 1}\n')
            with gating.fifo_slot(directory, 1, label='land', exclusive=True):
                self.assertFalse(stale.exists())
            live = queue / f'0-{os.getpid()}'
            live.write_text(f'{gating.process_start(os.getpid())}\n')
            with patch.object(gating.time, 'sleep', side_effect=KeyboardInterrupt):
                with self.assertRaises(KeyboardInterrupt):
                    with gating.fifo_slot(directory, 1, label='land', exclusive=True):
                        self.fail('overtook a live waiter')
            self.assertEqual(list(queue.iterdir()), [live])

    def test_land_rereads_the_count_each_poll_and_refuses_concurrent_lands(self):
        with tempfile.TemporaryDirectory() as scratch:
            directory = Path(scratch)

            def change_count(_):
                gating.write_gate_slot_count(directory, 2)

            with gating.fifo_slot(directory, 1, label='land', exclusive=True):
                with patch.object(gating.time, 'sleep', change_count):
                    with self.assertRaisesRegex(SystemExit, 'must be 1'):
                        with gating.fifo_slot(directory, 1, label='land', exclusive=True):
                            self.fail('concurrent lands admitted')
                self.assertEqual(list((directory / 'queue').iterdir()), [])

    def test_a_dead_ticket_is_removed_before_a_live_waiter_acquires(self):
        with tempfile.TemporaryDirectory() as scratch:
            queue = Path(scratch) / 'slots/queue'
            queue.mkdir(parents=True)
            exited = subprocess.Popen([sys.executable, '-c', 'pass'])
            self.assertEqual(exited.wait(), 0)
            dead = queue / f'0-{exited.pid}'
            dead.touch()
            with gating.gate_slot({gating.GATE_SLOTS: f'{scratch}/slots:1'}):
                self.assertFalse(dead.exists())
                self.assertEqual(list(queue.iterdir()), [])

    def test_an_interrupted_waiter_removes_its_ticket(self):
        with tempfile.TemporaryDirectory() as scratch:
            environ = {gating.GATE_SLOTS: f'{scratch}/slots'}
            gating.write_gate_slot_count(Path(scratch) / 'slots', 1)
            queue = Path(scratch) / 'slots/queue'

            def interrupt(_):
                self.assertEqual(len(list(queue.iterdir())), 1)
                raise KeyboardInterrupt

            with gating.gate_slot(environ), patch.object(gating.time, 'sleep', interrupt):
                with self.assertRaises(KeyboardInterrupt), contextlib.redirect_stdout(io.StringIO()):
                    with gating.gate_slot(environ):
                        self.fail('a waiter entered an occupied slot')
                self.assertEqual(list(queue.iterdir()), [])

    def test_no_cap_with_none_and_a_spec_without_a_directory_is_refused(self):
        none = {gating.GATE_SLOTS: gating.NO_SLOTS}
        with gating.gate_slot(none), gating.gate_slot(none), gating.gate_slot(none):
            pass
        for spec in ('', ':4'):
            with self.subTest(spec=spec):
                with self.assertRaises(SystemExit), gating.gate_slot({gating.GATE_SLOTS: spec}):
                    pass

    def test_status_names_holders_their_age_the_queue_and_this_worktrees_place(self):
        with tempfile.TemporaryDirectory() as scratch:
            directory = Path(scratch)
            gating.write_gate_slot_count(directory, 2)
            mine = os.getcwd()
            sleeper = subprocess.Popen([sys.executable, '-c', 'import time; time.sleep(30)'], cwd=scratch)
            self.addCleanup(sleeper.wait)
            self.addCleanup(sleeper.kill)
            queue = directory / 'queue'
            queue.mkdir()
            now = gating.time.time()
            (queue / f'{int((now - 600) * 1e9)}-{sleeper.pid}').write_text(f'{gating.process_start(sleeper.pid)}\n')
            (queue / f'{int((now - 40) * 1e9)}-{os.getpid()}').write_text(f'{gating.process_start(os.getpid())}\n')
            # A slot file whose holder exited is free.
            (directory / 'slot-1').write_text(f'{sleeper.pid} 1 /elsewhere\n')
            handle = open(directory / 'slot-0', 'a+')
            self.addCleanup(handle.close)
            fcntl.flock(handle, fcntl.LOCK_EX)
            handle.truncate(0)
            handle.write(f'{os.getpid()} {gating.process_start(os.getpid())} {mine}\n')
            handle.flush()
            os.utime(directory / 'slot-0', (now - 300, now - 300))
            lines = gating.queue_status(directory, me={mine}, now=now)
            self.assertEqual(lines[0], f'gate: normal gate slots: 2 in {directory}; 1 held, 2 queued')
            self.assertEqual(lines[1], f'  slot 0: held 5 min by {mine} (pid {os.getpid()}) (you)')
            self.assertEqual(lines[2], f'  queued 1: waiting 10 min, {scratch} (pid {sleeper.pid})')
            self.assertEqual(lines[3], f'  queued 2: waiting 40 s, {mine} (pid {os.getpid()}) (you)')
            self.assertEqual(lines[4:], ['gate: you hold normal gate slot 0',
                                         'gate: you are at 2 of 2 in the normal gate slot queue'])
            fcntl.flock(handle, fcntl.LOCK_UN)
            self.assertEqual(gating.queue_status(directory, me={mine}, now=now)[0],
                             f'gate: normal gate slots: 2 in {directory}; 0 held, 2 queued')


class Merging(unittest.TestCase):
    def manifest(self, *paths):
        return merging.canonical({'schema': 1, 'sources': [{'path': path, 'module': [path[:-4]]} for path in paths]})

    def test_generated_files_keep_mains_side_for_regeneration(self):
        for path in merging.REGENERATED:
            self.assertEqual(merging.resolve(path, 'a\n', 'main\n', 'lane\n'), 'main\n')

    def test_rows_both_sides_append_are_kept_mains_first(self):
        base = '| a |\n| b |\n'
        self.assertEqual(merging.resolve('third-party/NOTICES.md', base, base + '| main |\n', base + '| lane |\n'),
                         base + '| main |\n| lane |\n')

    def test_a_row_both_sides_edit_stays_a_conflict(self):
        self.assertIsNone(merging.resolve('third-party/NOTICES.md', '| a |\n', '| main |\n', '| lane |\n'))

    def test_manifest_sources_both_sides_add_are_kept_and_removals_hold(self):
        base = self.manifest('a.hir', 'b.hir')
        ours = self.manifest('a.hir', 'b.hir', 'main.hir')
        theirs = self.manifest('b.hir', 'lane.hir')
        self.assertEqual(merging.resolve('stdlib/std@1/hirsel.json', base, ours, theirs),
                         self.manifest('b.hir', 'main.hir', 'lane.hir'))

    def test_manifest_edits_of_one_value_or_entry_stay_conflicts(self):
        base = self.manifest('a.hir')
        self.assertIsNone(merging.resolve('stdlib/std@1/hirsel.json', base, self.manifest('main.hir'),
                                          self.manifest('lane.hir')))
        schema = merging.canonical({'schema': 2})
        self.assertIsNone(merging.resolve('packages/x/hirsel.json', merging.canonical({'schema': 1}), schema,
                                          merging.canonical({'schema': 3})))

    def test_a_manifest_not_in_canonical_form_and_other_files_stay_conflicts(self):
        base = self.manifest('a.hir')
        self.assertIsNone(merging.resolve('stdlib/std@1/hirsel.json', base, base.replace('  ', '    '),
                                          self.manifest('a.hir', 'lane.hir')))
        self.assertIsNone(merging.resolve('README.md', 'a\n', 'b\n', 'c\n'))
        self.assertIsNone(merging.resolve('third-party/NOTICES.md', 'a\n', None, 'c\n'))


class Summary(unittest.TestCase):
    def test_a_failing_case_names_the_command_that_reruns_it_alone(self):
        details = ('running 2 tests\ntest native::a ... ok\ntest native::b ... FAILED\n\nfailures:\n\n'
                   '---- native::b stdout ----\nthread panicked\n')
        self.assertEqual(gating.rerun('hirsel-driver-test-it-shard-3', details), [
            'native::b\n    .buck2/bin/buck2 test //:hirsel-driver-test-it-shard-1 -- '
            '--test-arg=native::b --test-arg=--exact'])
        self.assertEqual(gating.rerun('hirsel-lexer-unittest', 'test x ... FAILED\n')[0].split('\n    ')[1],
                         '.buck2/bin/buck2 test //:hirsel-lexer-unittest -- --test-arg=x --test-arg=--exact')
        self.assertEqual(gating.rerun('hirsel-lir', 'doctest failed'), ['.buck2/bin/buck2 test //:hirsel-lir'])

    def test_a_test_the_pool_never_ran_says_so(self):
        details = ('---- STDOUT ----\n\n---- STDERR ----\nRemote Execution Error on Execute with digest 02:146\n'
                   "Error: (Error was returned on the stream by RE: Command 'x' timed out after 6 seconds)\n")
        lines = gating.rerun('gate-conformance-shard-1', details)
        self.assertTrue(lines[0].startswith('the pool never ran it: Remote Execution Error'), lines)
        self.assertEqual(lines[1], '.buck2/bin/buck2 test //:gate-conformance-shard-1')

    def test_campaign_failures_name_their_programs(self):
        conformance = ('---- STDERR ----\nconformance/MODE/MODE-7-run.hir: corpus source is not '
                       'formatter-canonical; run corpus-fmt with the release hirsel\n')
        self.assertEqual(gating.rerun('gate-conformance-shard-6', conformance), [
            'conformance/MODE/MODE-7-run.hir: corpus source is not formatter-canonical; run corpus-fmt with the release hirsel',
            '.buck2/bin/buck2 test -c hirsel.tests=focus //:gate-conformance-shard-1 -- --test-arg=--shard '
            '--test-arg=1/1 --test-arg=--path --test-arg=conformance/MODE/MODE-7-run.hir',
            '.buck2/bin/buck2 test //:gate-conformance-shard-6'])
        agreement = json.dumps({'checked': 3, 'clusters': [{'kind': 'source accepts', 'path': 'conformance/x.hir'}]})
        for target in ['gate-agree-shard-2', 'gate-agree-selected-shard-1']:
            lines = gating.rerun(target, agreement)
            self.assertEqual(lines[0], 'source accepts: conformance/x.hir')
            # The gate's own sample within the case reproduces the variant; --program
            # would replay the unmutated case.
            self.assertTrue(lines[1].endswith('-- --root "$PWD" --seed 1 --limit 5000 --sample-path conformance/x.hir'),
                            lines[1])
            self.assertNotIn('--program', lines[1])
            self.assertEqual(lines[-1], f'.buck2/bin/buck2 test //:{target}')


class TestResultHistory(unittest.TestCase):
    def test_every_result_is_recorded_with_gate_target_shard_duration_and_date(self):
        records = []
        class SimulatedHistory:
            def append(self, rows):
                records.extend(rows)
        def event(name, duration, status):
            return json.dumps({'Event': {'timestamp': [0, 0], 'data': {'Instant': {'data': {
                'TestResult': {'name': name, 'duration_us': duration, 'status': status,
                               'details': 'test macro::case ... ok <0.123s>' if status == gating.PASS else ''}}}}}})
        output = '\n'.join([event('root//:driver-shard-2 (config)', 1250000, gating.PASS),
                            event('root//:whole', 500000, gating.FAIL)])
        with patch.object(gating.subprocess, 'run', return_value=subprocess.CompletedProcess([], 0, output)):
            results = gating.test_results(Path('/logs/gate-123.pb.zst'), history=SimulatedHistory())
        self.assertEqual(results['driver-shard-2']['seconds'], 1.25)
        self.assertEqual(records, [dict(gate='gate-123', target='driver', shard='2', duration_seconds=1.25,
                                      date='1970-01-01T00:00:00+00:00', status=gating.PASS,
                                      case_costs='{"macro::case":0.123}'),
                                   dict(gate='gate-123', target='whole', shard='', duration_seconds=0.5,
                                      date='1970-01-01T00:00:00+00:00', status=gating.FAIL, case_costs='')])


FAKE_BUCK2 = '''#!/usr/bin/env bash
isolation=${BUCK_ISOLATION_DIR:-v2}
if [[ $1 == --isolation-dir ]]; then isolation=$2; shift 2; fi
if [[ $1 == kill ]]; then
  [[ -n ${DAEMON_TRACE:-} ]] && printf 'kill %s %s\\n' "$isolation" "$PWD" >> "$DAEMON_TRACE"
  exit "${FAIL_KILL:-0}"
fi
[[ -n ${DAEMON_TRACE:-} ]] && printf '%s %s %s\\n' "$1" "$isolation" "$PWD" >> "$DAEMON_TRACE"
if [[ $1 == build && -n ${READY_PIPE:-} ]]; then
  bash -c 'echo "$BASHPID" > "$CHILD_PID"; printf "ready\\n" > "$READY_PIPE"; read -r < "$BLOCK_PIPE"'
fi
# Each test target passes with the counts a conformance shard prints, unless
# FAIL_TESTS is set; `log show` replays the results of the run that wrote them.
details='whole corpus: 1 validated / 1 passing / 0 failing / 0 not implemented\\nin-gate corpus: 1 validated / 1 passing / 0 failing / 0 not implemented\\ncorpus formatting: 0 intentional syntax rejections exempt'
log=
arguments=("$@")
for ((i = 0; i < $#; i++)); do [[ ${arguments[i]} == --event-log ]] && log=${arguments[i + 1]}; done
[[ -n $log ]] && printf '{}' > "$log"
if [[ ${2:-} == --event-log ]]; then
  printf '%s\\n' "$1 ${*:4}" >> "$COMMANDS"
else
  printf '%s\\n' "$*" >> "$COMMANDS"
fi
if [[ $1 == test && -n $log ]]; then
  for argument in "$@"; do
    [[ $argument == -- ]] && break
    [[ $argument == "${OMIT_TARGET:-}" ]] && continue
    [[ $argument == root//* ]] && printf '{"Event":{"data":{"Instant":{"data":{"TestResult":{"name":"%s","status":%s,"details":"%s","duration_us":1000}}}}}}\\n' \\
      "$argument" "$( [[ -n ${FAIL_TESTS:-} || $argument == "${FAIL_TARGET:-}" ]] && echo 2 || echo 1 )" "$details" >> "$log.results"
  done
fi
# A tree holding FAIL_BUILD_IF fails its workspace build (a batch member that turns main red).
if [[ $1 == build && -n ${FAIL_BUILD_IF:-} && -e $FAIL_BUILD_IF && $* != *show-json-output* ]]; then
  echo "error: $FAIL_BUILD_IF breaks the build" >&2; exit 1
fi
case $1 in
  log) [[ -f $3.results ]] && cat "$3.results"; exit 0 ;;
  uquery) query=$*; last=${*: -1}; [[ $last == @* ]] && query=$(cat "${last#@}")
    [[ $query == *'kind("^rust_'* ]] && echo 'root//:a-clippy' || echo 'root//:a'; exit "${FAIL_QUERY:-0}" ;;
  build)
    if [[ $* == *show-json-output* ]]; then
      echo '{"root//:hirsel-cli-bin-hirsel": ".buck2/bin/hirsel"}'
      exit "${FAIL_BUILD:-0}"
    fi
    if [[ $* == *show-full-json-output* && -n ${EXPIRED_ONCE:-} && ! -e $EXPIRED_ONCE ]]; then
      touch "$EXPIRED_ONCE"
      echo 'Error materializing artifact: retrieved 349 seconds ago with ttl = 0 seconds' >&2
      exit 1
    fi
    [[ $* == *show-full-json-output* ]] && echo '{}'; exit "${FAIL_BUILD:-0}" ;;
esac
'''
FAKE_SCRIPT = '''import os, sys
CONFORMANCE_SHARDS = 16
AGREE_SHARDS = 16
AGREE_LIMIT = 5000
AGREE_FULL_SHARDS = 96
AGREE_FULL_TIMEOUT = 1800
AGREE_NATIVE_FULL_SHARDS = 16
AGREE_NATIVE_FULL_TIMEOUT = 3600
CONFORMANCE_SLICE_PROGRAMS = 20
AGREE_SLICE_PROGRAMS = 260
SELECTED_SLICES = 16
if __name__ == "__main__":
    if "{name}" == "prepare.py" and os.environ.get("FAKE_BUCK2_BIN") and not os.path.exists(".buck2/bin"):
        # The batch worktree's Buck2 and CLI, as bootstrap would install them.
        os.makedirs(".buck2", exist_ok=True)
        os.symlink(os.environ["FAKE_BUCK2_BIN"], ".buck2/bin")
    if os.environ.get("LOCK_TRACE"):
        import fcntl
        from pathlib import Path
        with open(Path(os.environ["HIRSEL_LAND_LOCK_DIR"]) / "slot-0", "a+") as lock:
            try:
                fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
                held = False
            except BlockingIOError:
                held = True
        with open(os.environ["LOCK_TRACE"], "a") as log:
            print("{name}", held, file=log)
    with open(os.environ["COMMANDS"], "a") as log:
        print(" ".join(["{name}", *sys.argv[1:]]), file=log)
    sys.exit(int(os.environ.get("FAIL_{upper}", "0")))
'''

FAKE_HIRSEL = '''#!/usr/bin/env python3
import fcntl
import os
from pathlib import Path
import subprocess
import sys
if os.environ.get("ASSERT_LAND_LOCK"):
    with open(Path(os.environ["HIRSEL_LAND_LOCK_DIR"]) / "slot-0", "a+") as lock:
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            pass
        else:
            sys.exit("index generation ran outside the land lock")
with open(os.environ["COMMANDS"], "a") as log:
    print("hirsel " + " ".join(sys.argv[1:]), file=log)
if os.environ.get("FAIL_INDEX"):
    print("partial output")
    print("index generation failed", file=sys.stderr)
    sys.exit(1)
if os.environ.get("INDEX_PUSH_ONCE"):
    marker = Path(os.environ["INDEX_PUSH_MARKER"])
    if not marker.exists():
        marker.touch()
        subprocess.run(["git", "-C", os.environ["INDEX_PUSH_ONCE"], "push", "-q", "origin", "HEAD:main"],
                       check=True, capture_output=True)
sys.stdout.write(os.environ.get("INDEX_TEXT", Path("docs/catalogue.md").read_text()))
'''

FAKE_SCOPE = '''#!/usr/bin/env python3
import os, sys
with open(os.environ["COMMANDS"], "a") as log:
    print("systemd-run " + " ".join(sys.argv[1:]), file=log)
assert sys.argv[1:8] == ["--user", "--scope", "-q", "-p", "MemoryMax=2560M", "-p", "RuntimeMaxSec=300"]
assert os.environ["XDG_RUNTIME_DIR"]
if os.environ.get("FAIL_SCOPE"):
    print("scope killed by resource limit", file=sys.stderr)
    sys.exit(137)
os.execv(sys.argv[8], sys.argv[8:])
'''


class Scripts(unittest.TestCase):
    """tools/gate and tools/land in a scratch clone with a fake Buck2 and fake helpers."""

    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        top = Path(self.temp.name)
        self.origin = top / 'origin.git'
        self.root = top / 'lane'
        self.root.mkdir()
        scratch_repo(self.root)
        files = {'Cargo.toml': '[workspace]\nmembers = ["crates/*"]\n', 'BUCK': SCRIPT_BUCK, 'README.md': 'Hirsel\n',
                 'docs/building.md': 'See [the grammar](grammar.md).\n', 'docs/grammar.md': 'grammar\n',
                 'docs/api.md': 'index\n', 'docs/catalogue.md': 'index\n',
                 **{f'crates/{crate}/Cargo.toml': f'[package]\nname = "{crate}"\n' for crate in impact.CRATE_CAMPAIGNS},
                 '.gitignore': (REPO / '.gitignore').read_text()}
        for name in ('gate', 'land'):
            files[f'tools/{name}'] = (TOOLS / name).read_text()
        for name in ('gating', 'graph', 'impact', 'merging', 'clippy', 'agree', 'conformance', 'event_logs'):
            files[f'tools/buck2/{name}.py'] = (TOOLS / 'buck2' / f'{name}.py').read_text()
        for name in ('sync', 'prepare', 'fmt'):
            files[f'tools/buck2/{name}.py'] = FAKE_SCRIPT.replace('{name}', f'{name}.py').replace('{upper}', name.upper())
        files['tools/tests/ignored_tests.py'] = FAKE_SCRIPT.replace('{name}', 'ignored_tests.py').replace('{upper}', 'IGNORED')
        commit(self.root, files, 'Base')
        for name in ('gate', 'land'):
            (self.root / 'tools' / name).chmod(0o755)
        git(self.root, 'commit', '-q', '-am', 'Executable tools')
        buck2 = self.root / '.buck2/bin/buck2'
        buck2.parent.mkdir(parents=True)
        fake_bin = top / 'bin'
        fake_bin.mkdir()
        fake_buck2 = fake_bin / 'buck2'
        fake_buck2.write_text(FAKE_BUCK2)
        fake_buck2.chmod(0o755)
        scope = fake_bin / 'systemd-run'
        scope.write_text(FAKE_SCOPE)
        scope.chmod(0o755)
        buck2.symlink_to(fake_buck2)
        hirsel = buck2.with_name('hirsel')
        hirsel.write_text(FAKE_HIRSEL)
        hirsel.chmod(0o755)
        subprocess.run(['git', 'clone', '-q', '--bare', str(self.root), str(self.origin)], check=True)
        git(self.root, 'remote', 'add', 'origin', str(self.origin))
        git(self.root, 'fetch', '-q', 'origin')
        git(self.root, 'branch', '-q', '--set-upstream-to=origin/main')
        self.commands = top / 'commands'
        self.env = os.environ | {'COMMANDS': str(self.commands),
                                 'PATH': str(fake_bin) + os.pathsep + os.environ['PATH'],
                                 'HIRSEL_LAND_LOCK_DIR': str(top / 'land-lock'),
                                 'FAKE_BUCK2_BIN': str(self.root / '.buck2/bin'),
                                 'HIRSEL_BUCK2_LOG_DIR': str(self.root / '.buck2/event-logs')}
        # Scratch gates must not queue for the host's slots, nor acquire one
        # held by the gate running this test.
        self.env[gating.GATE_SLOTS] = gating.NO_SLOTS
        self.env.pop('HIRSEL_KEEP_DAEMON', None)
        self.env.pop('BUCK_ISOLATION_DIR', None)
        # A sibling's clone, to move main.
        self.sibling = top / 'sibling'
        subprocess.run(['git', 'clone', '-q', str(self.origin), str(self.sibling)], check=True)
        git(self.sibling, 'config', 'user.email', 'test@example.invalid')
        git(self.sibling, 'config', 'user.name', 'Sibling')

    def run_tool(self, *command, env=None):
        self.commands.unlink(missing_ok=True)
        result = subprocess.run([str(self.root / 'tools' / command[0]), *command[1:]], cwd=self.root,
                                env=self.env | (env or {}), capture_output=True, text=True)
        logged = self.commands.read_text().splitlines() if self.commands.exists() else []
        return result, logged

    def test_buck_backed_sync_starts_after_preparation_and_the_prequeue_daemon_stop(self):
        path = self.root / 'tools/buck2/gating.py'
        tracing = '''
_start_local_check = Local.start
def traced_local_check(self, name, command):
    if any(str(arg).endswith('sync.py') for arg in command):
        with open(os.environ['DAEMON_TRACE'], 'a') as trace:
            trace.write('sync-start\\n')
    return _start_local_check(self, name, command)
Local.start = traced_local_check
'''
        commit(self.root, {'tools/buck2/gating.py': path.read_text() + tracing,
                           'bench/native/tests/test_fixture.py':
                               'import unittest\nclass Fixture(unittest.TestCase):\n    def test_available(self): pass\n'},
               'Trace sync lifecycle')
        trace = self.root.parent / 'daemon-trace'
        result, _ = self.run_tool('gate', '--tests', 'suggested', '--why', 'Buck-backed inventory',
                                  env={'DAEMON_TRACE': str(trace)})
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        rows = trace.read_text().splitlines()
        sync = rows.index('sync-start')
        build = next(i for i, row in enumerate(rows) if row.startswith('build '))
        stop = next(i for i, row in enumerate(rows) if row.startswith('kill v2 '))
        self.assertGreater(sync, build, 'inventory started before the prepared workspace build')
        # The inventory is in the floor: it finishes before the gate stops its daemon to queue.
        self.assertLess(sync, stop, 'inventory was live when the gate stopped its daemon')

    def sibling_pushes(self, files):
        commit(self.sibling, files, 'Sibling change')
        git(self.sibling, 'push', '-q', 'origin', 'HEAD:main')

    def daemon_change(self, tool):
        if tool == 'land':
            commit(self.root, {'README.md': 'Daemon lifecycle fixture\n'}, 'Docs')
            result, _ = self.run_tool('gate', '--why', 'docs only')
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            return ['land']
        commit(self.root, {'crates/hirsel-span/tests/it/main.rs': '#[test]\nfn t() {}\n'}, 'Test')
        return ['gate', '--why', 'one test target']

    def daemon_trace(self):
        path = self.root.parent / 'daemon-trace'
        return path.read_text().splitlines() if path.exists() else []

    def daemon_env(self):
        return {'DAEMON_TRACE': str(self.root.parent / 'daemon-trace')}

    def assert_daemons_stopped(self):
        self.assertEqual(self.daemon_trace()[-2:],
                         [f'kill v2 {self.root}', f'kill buck2-bootstrap {self.root}'])

    def test_daemons_stop_on_success(self):
        for tool in ('gate', 'land'):
            with self.subTest(tool=tool):
                arguments = self.daemon_change(tool)
                result, _ = self.run_tool(*arguments, env=self.daemon_env())
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                self.assert_daemons_stopped()

    def test_daemons_stop_on_failure(self):
        for tool in ('gate', 'land'):
            with self.subTest(tool=tool):
                arguments = self.daemon_change(tool)
                result, _ = self.run_tool(*arguments, env=self.daemon_env() | {'FAIL_BUILD': '1'})
                self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
                self.assert_daemons_stopped()

    def test_cleanup_attempts_every_isolation_even_when_kill_fails(self):
        result, _ = self.run_tool('gate', '--why', 'no change',
                                  env=self.daemon_env() | {'FAIL_KILL': '1'})
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('Buck2 daemon cleanup failed', result.stderr)
        self.assert_daemons_stopped()

    def test_suggestion_is_static_and_leaves_the_worktrees_daemon_alone(self):
        self.daemon_change('gate')
        result, logged = self.run_tool('gate', '--suggest', env=self.daemon_env() | {'FAIL_QUERY': '1'})
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual((logged, self.daemon_trace()), ([], []))
        self.assertIn('1 test targets: a', result.stdout)

    def test_interactive_keep_daemon_is_an_explicit_opt_out(self):
        for tool in ('gate', 'land'):
            with self.subTest(tool=tool):
                arguments = self.daemon_change(tool)
                trace = self.root.parent / 'daemon-trace'
                trace.unlink(missing_ok=True)
                result, _ = self.run_tool(*arguments, env=self.daemon_env() | {'HIRSEL_KEEP_DAEMON': '1'})
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                self.assertTrue(self.daemon_trace())
                self.assertFalse(any(row.startswith('kill ') for row in self.daemon_trace()))

    @contextlib.contextmanager
    def daemon_process(self, arguments, env):
        with subprocess.Popen([str(self.root / 'tools' / arguments[0]), *arguments[1:]],
                              cwd=self.root, env=self.env | self.daemon_env() | env,
                              stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True) as process:
            try:
                yield process
            finally:
                if process.poll() is None:
                    process.kill()
                process.communicate(timeout=10)

    def test_daemons_stop_on_interrupt_during_a_buck2_command(self):
        for tool in ('gate', 'land'):
            arguments = self.daemon_change(tool)
            for sig in (signal.SIGINT, signal.SIGTERM, signal.SIGHUP):
                with self.subTest(tool=tool, signal=sig):
                    ready, block = [self.root.parent / name for name in ('ready', 'block')]
                    for path in (ready, block):
                        path.unlink(missing_ok=True)
                        os.mkfifo(path)
                    descriptor = os.open(ready, os.O_RDWR | os.O_NONBLOCK)
                    try:
                        with selectors.DefaultSelector() as selector:
                            selector.register(descriptor, selectors.EVENT_READ)
                            with self.daemon_process(arguments, {'READY_PIPE': str(ready),
                                                                 'BLOCK_PIPE': str(block),
                                                                 'CHILD_PID': str(self.root.parent / 'child-pid')}) as process:
                                self.assertTrue(selector.select(timeout=10), 'Buck2 never reached the build')
                                self.assertEqual(os.read(descriptor, 100), b'ready\n')
                                child = os.pidfd_open(int((self.root.parent / 'child-pid').read_text()))
                                try:
                                    selector.unregister(descriptor)
                                    selector.register(child, selectors.EVENT_READ)
                                    process.send_signal(sig)
                                    out, err = process.communicate(timeout=10)
                                    self.assertEqual(process.returncode, 128 + sig, out + err)
                                    self.assertTrue(selector.select(timeout=10), 'Buck2 helper descendant survived')
                                    self.assert_daemons_stopped()
                                finally:
                                    os.close(child)
                    finally:
                        os.close(descriptor)

    def test_queued_tools_hold_no_daemon_and_restart_after_acquisition(self):
        for tool in ('gate', 'land'):
            arguments = self.daemon_change(tool)
            # Interrupted first: a land that completes leaves nothing to land.
            for interrupt in (True, False):
                with self.subTest(tool=tool, interrupt=interrupt):
                    directory = self.root.parent / ('slots' if tool == 'gate' else 'land-lock')
                    env = {gating.GATE_SLOTS: f'{directory}:1'} if tool == 'gate' else {}
                    pool = gating.EXPRESS_POOL if tool == 'gate' else gating.NORMAL_POOL
                    with contextlib.ExitStack() as held:
                        # Fill both express slots; land always has one exclusive slot.
                        for _ in range(2 if tool == 'gate' else 1):
                            held.enter_context(gating.fifo_slot(directory, 2 if tool == 'gate' else 1, pool=pool))
                        with self.daemon_process(arguments, env) as process:
                            with selectors.DefaultSelector() as selector:
                                selector.register(process.stdout, selectors.EVENT_READ)
                                # Read bytes directly: TextIO buffering would hide lines from select.
                                output = b''
                                while (b'; held by:' if tool == 'gate' else b'land: waiting;') not in output:
                                    self.assertTrue(selector.select(timeout=10), output.decode())
                                    chunk = os.read(process.stdout.fileno(), 4096)
                                    self.assertTrue(chunk, output.decode())
                                    output += chunk
                            self.assert_daemons_stopped()
                            before = len(self.daemon_trace())
                            if interrupt:
                                process.terminate()
                            else:
                                held.close()
                            out, err = process.communicate(timeout=10)
                            self.assertEqual(process.returncode, 143 if interrupt else 0, out + err)
                            self.assert_daemons_stopped()
                            if not interrupt:
                                # A gate built before it queued; after the slot it only tests.
                                first = 'test ' if tool == 'gate' else 'build '
                                self.assertTrue(any(row.startswith(first) for row in self.daemon_trace()[before:]))
                    self.assertEqual(list((directory / (gating.POOL_PREFIX[pool] + 'queue')).iterdir()), [])

    def test_set_slots_updates_each_host_count_without_checks_or_a_slot(self):
        directory = self.root.parent / 'slots'
        env = {gating.GATE_SLOTS: f'{directory}:1'}
        for option, pool in (('--set-slots', gating.NORMAL_POOL), ('--set-express-slots', gating.EXPRESS_POOL)):
            with self.subTest(pool=pool):
                path = directory / (gating.POOL_PREFIX[pool] + 'count')
                result, logged = self.run_tool('gate', option, '2', env=env)
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                self.assertEqual(logged, [])
                self.assertEqual(path.read_text(), '2\n')
                with gating.gate_slot(env, pool=pool), gating.gate_slot(env, pool=pool):
                    result, logged = self.run_tool('gate', option, '1', env=env)
                    self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                    self.assertEqual(logged, [])
                    self.assertEqual(path.read_text(), '1\n')
        self.assertEqual((directory / 'count').read_text(), '1\n')
        self.assertEqual(list(directory.glob('.*count-*')), [])

    def test_status_runs_no_buck2_and_leaves_the_worktrees_daemon_running(self):
        directory = self.root.parent / 'slots'
        with gating.gate_slot({gating.GATE_SLOTS: str(directory)}):
            result, logged = self.run_tool('gate', '--status', env={gating.GATE_SLOTS: f'{directory}:4',
                                                                    **self.daemon_env()})
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual((logged, self.daemon_trace()), ([], []))
        self.assertIn(f'gate: normal gate slots: 6 in {directory}; 1 held, 0 queued', result.stdout)
        self.assertIn('gate: express gate slots: 2', result.stdout)

    def test_set_slots_refuses_invalid_counts_missing_configuration_and_gate_options(self):
        directory = self.root.parent / 'slots'
        env = {gating.GATE_SLOTS: f'{directory}:2'}
        for option in ('--set-slots', '--set-express-slots'):
            for arguments, config in [((option, '0'), env), ((option, '-1'), env),
                                      ((option, 'six'), env), ((option, '2'), {}),
                                      ((option, '2', '--suggest'), env),
                                      ((option, '2', '--why', 'check'), env),
                                      (('--set-slots', '2', '--set-express-slots', '2'), env)]:
                with self.subTest(arguments=arguments, config=config):
                    result, logged = self.run_tool('gate', *arguments, env=config)
                    self.assertEqual(result.returncode, 2, result.stdout + result.stderr)
                    self.assertEqual(logged, [])
                    self.assertFalse(directory.exists())

    def test_a_small_resolved_gate_uses_express_and_records_it_while_normal_is_full(self):
        commit(self.root, {'crates/hirsel-span/tests/it/main.rs': '#[test]\nfn t() {}\n'}, 'Test')
        directory = self.root.parent / 'slots'
        env = {gating.GATE_SLOTS: f'{directory}:6'}
        with contextlib.ExitStack() as held:
            for _ in range(6):
                held.enter_context(gating.gate_slot(env))
            # Bound the subprocess so accidentally choosing normal fails rather than hangs.
            self.env.update(env)
            result = subprocess.run([str(self.root / 'tools/gate'), '--why', 'one test target'],
                                    cwd=self.root, env=self.env, capture_output=True, text=True, timeout=15)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn('took express gate slot', result.stdout)
        _, record = gating.latest_green(self.root)
        self.assertEqual(record['pool'], 'express')
        result, _ = self.run_tool('land')
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn('pool: express', result.stdout)
        [landing] = (self.root / '.git/hirsel-landings.jsonl').read_text().splitlines()
        self.assertEqual(json.loads(landing)['pool'], 'express')

    def test_documentation_alone_runs_no_buck2_and_catches_broken_links(self):
        commit(self.root, {'docs/building.md': 'See [the grammar](grammar.md) and [x](gone.md).\n'}, 'Docs')
        result, logged = self.run_tool('gate', '--why', 'scratch change')
        self.assertEqual(result.returncode, 1, result.stdout)
        self.assertIn('docs/building.md: link gone.md names nothing', result.stdout)
        self.assertEqual(logged, [])
        commit(self.root, {'docs/building.md': 'See [the grammar](grammar.md).\nMore.\n'}, 'Docs fixed')
        result, logged = self.run_tool('gate', '--why', 'scratch change')
        self.assertEqual(result.returncode, 0, result.stdout)
        self.assertEqual(logged, [])
        self.assertEqual(gating.latest_green(self.root)[1]['pool'], 'none')
        # Moving a linked document without its links fails.
        commit(self.root, {'docs/grammar.md': None, 'docs/syntax.md': 'grammar\n'}, 'Move')
        result, _ = self.run_tool('gate', '--why', 'scratch change')
        self.assertIn('docs/building.md: link grammar.md names a deleted path', result.stdout)

    def test_land_refuses_an_ungated_change_and_lands_a_gated_one_after_main_moves(self):
        commit(self.root, {'README.md': 'Hirsel, a language\n'}, 'Readme')
        result, logged = self.run_tool('land')
        self.assertEqual(result.returncode, 1)
        self.assertIn('tools/gate has not passed this change', result.stdout)
        self.assertFalse(any(c.startswith('build') for c in logged))
        self.assertFalse(Path(self.env['HIRSEL_LAND_LOCK_DIR']).exists())
        self.assertEqual(self.run_tool('gate', '--why', 'scratch change')[0].returncode, 0)
        self.sibling_pushes({'LICENSE': 'MIT\n'})
        result, logged = self.run_tool('land')
        self.assertEqual(result.returncode, 0, result.stdout)
        self.assertIn('land: pushed', result.stdout)
        self.assertEqual(git(self.origin, 'rev-parse', 'main'), git(self.root, 'rev-parse', 'HEAD'))
        self.assertEqual(git(self.origin, 'show', 'main:LICENSE'), 'MIT')
        # Build and Clippy in one Buck2 command, formatting and build files beside it; no test.
        self.assertIn('build -c hirsel.clippy=true -c buck2.test_builds_targets=true --materializations none '
                      'root//:a root//:a-clippy[clippy.txt]', logged)
        self.assertIn('fmt.py --check', logged)
        self.assertIn('sync.py --check', logged)
        self.assertFalse(any(c.startswith('test') for c in logged))
        self.assertNotIn('Regenerate the API index.', git(self.root, 'log', '--format=%s'))
        self.assertGreaterEqual(len(list((self.root / '.buck2/event-logs').glob('*.pb.zst'))), 2)
        [landing] = (self.root / '.git/hirsel-landings.jsonl').read_text().splitlines()
        timing = json.loads(landing)
        self.assertGreaterEqual(timing['land_wait_seconds'], 0)
        self.assertGreater(timing['land_held_seconds'], 0)
        self.assertIn('after waiting', result.stdout)
        self.assertIn('held land lock for', result.stdout)

    def test_land_fetches_prepares_checks_and_pushes_inside_the_lock(self):
        commit(self.root, {'README.md': 'Hirsel, a language\n'}, 'Readme')
        self.assertEqual(self.run_tool('gate', '--why', 'scratch change')[0].returncode, 0)
        real_git = shutil.which('git')
        wrapper = self.root / '.buck2/bin/git'
        trace = self.root.parent / 'lock-trace'
        wrapper.write_text('''#!/usr/bin/env python3
import fcntl, os, sys
from pathlib import Path
with open(Path(os.environ['HIRSEL_LAND_LOCK_DIR']) / 'slot-0', 'a+') as lock:
    try:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        held = False
    except BlockingIOError:
        held = True
with open(os.environ['LOCK_TRACE'], 'a') as log:
    print(sys.argv[1], held, file=log)
os.execv(os.environ['REAL_GIT'], [os.environ['REAL_GIT'], *sys.argv[1:]])
''')
        wrapper.chmod(0o755)
        directory = Path(self.env['HIRSEL_LAND_LOCK_DIR'])
        directory.mkdir()
        result, logged = self.run_tool('land', env={
            'PATH': str(wrapper.parent) + os.pathsep + self.env['PATH'],
            'REAL_GIT': real_git, 'LOCK_TRACE': str(trace), 'ASSERT_LAND_LOCK': '1',
        })
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        rows = trace.read_text().splitlines()
        for command in ('fetch', 'rebase', 'push'):
            self.assertIn(command + ' True', rows)
        for command in ('rebase', 'push'):
            self.assertNotIn(command + ' False', rows)
        # One fetch before the queue finds main's copy of the landing scripts.
        self.assertEqual(rows.count('fetch False'), 1)
        self.assertLess(rows.index('fetch False'), rows.index('archive False'))
        self.assertIn('status False', rows)
        # The batch worktree is prepared once, under the lock, before any Buck2 command.
        self.assertEqual([row for row in rows if row.startswith('prepare.py')], ['prepare.py True'])
        self.assertIn('fmt.py True', rows)
        self.assertIn('sync.py True', rows)
        self.assertEqual(logged[0], 'prepare.py')
        self.assertEqual(logged.count('prepare.py'), 1)

    def test_land_runs_main_s_copy_of_itself_on_the_worktree(self):
        commit(self.root, {'README.md': 'Hirsel, a language\n'}, 'Readme')
        self.assertEqual(self.run_tool('gate', '--why', 'scratch change')[0].returncode, 0)
        land = (self.sibling / 'tools/land').read_text()
        marker = "    say('land: main copy marker ' + str(ROOT))\n"
        land = land.replace('    os.chdir(ROOT)\n', '    os.chdir(ROOT)\n' + marker, 1)
        self.sibling_pushes({'tools/land': land})
        result, _ = self.run_tool('land')
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("running origin/main's copy of the landing scripts", result.stdout)
        # The worktree's stale copy ran once, to start main's; main's ran on the worktree.
        self.assertEqual(result.stdout.count(f'land: main copy marker {self.root}'), 1, result.stdout)
        self.assertIn('pushed', result.stdout)
        self.assertEqual(git(self.root, 'rev-parse', 'HEAD'), git(self.origin, 'rev-parse', 'main'))

    def test_land_refuses_to_run_under_an_outer_timeout(self):
        commit(self.root, {'README.md': 'Hirsel, a language\n'}, 'Readme')
        self.assertEqual(self.run_tool('gate', '--why', 'scratch change')[0].returncode, 0)
        self.commands.unlink(missing_ok=True)
        result = subprocess.run(['timeout', '60', str(self.root / 'tools/land')], cwd=self.root, env=self.env,
                                capture_output=True, text=True)
        self.assertEqual(result.returncode, 2, result.stdout + result.stderr)
        self.assertIn('refusing to run under timeout', result.stdout)
        self.assertFalse(self.commands.exists())
        self.assertNotEqual(git(self.root, 'rev-parse', 'HEAD'), git(self.origin, 'rev-parse', 'main'))

    def test_land_status_names_the_holder_its_age_and_the_waiters_without_buck2(self):
        directory = Path(self.env['HIRSEL_LAND_LOCK_DIR'])
        with gating.fifo_slot(directory, 1, label='land', exclusive=True):
            result, logged = self.run_tool('land', '--status', env=self.daemon_env())
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual((logged, self.daemon_trace()), ([], []))
        self.assertIn(f'land: land lock: 1 in {directory}; 1 held, 0 queued', result.stdout)
        self.assertRegex(result.stdout, r'slot 0: held \d+ s by ')

    def test_land_commits_the_index_generated_from_the_rebased_sources(self):
        commit(self.root, {'README.md': 'Hirsel, a language\n'}, 'Readme')
        self.assertEqual(self.run_tool('gate', '--why', 'scratch change')[0].returncode, 0)
        self.sibling_pushes({'docs/catalogue.md': 'a new export\n'})
        records = gating.green_records(self.root)
        result, logged = self.run_tool('land')
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(git(self.origin, 'show', 'main:docs/api.md'), 'a new export')
        self.assertEqual(git(self.root, 'log', '-1', '--format=%s'), 'Regenerate the API index.')
        self.assertEqual(git(self.root, 'diff-tree', '--no-commit-id', '--name-only', '-r', 'HEAD'), 'docs/api.md')
        self.assertEqual(gating.green_records(self.root), records)
        self.assertIn('hirsel doc stdlib/std@1/hirsel.json', logged)
        self.assertFalse(any(c.startswith('test') for c in logged))

    def test_land_does_not_push_or_replace_the_page_when_index_generation_fails(self):
        commit(self.root, {'README.md': 'Hirsel, a language\n'}, 'Readme')
        self.assertEqual(self.run_tool('gate', '--why', 'scratch change')[0].returncode, 0)
        before = git(self.origin, 'rev-parse', 'main')
        result, _ = self.run_tool('land', env={'FAIL_INDEX': '1'})
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn('index generation failed', result.stdout)
        self.assertEqual(git(self.origin, 'rev-parse', 'main'), before)
        self.assertEqual((self.root / 'docs/api.md').read_text(), 'index\n')
        self.assertEqual(git(self.root, 'status', '--porcelain'), '')

    def test_land_caps_the_index_generator_and_refuses_a_scope_kill(self):
        commit(self.root, {'README.md': 'Hirsel, a language\n'}, 'Readme')
        self.assertEqual(self.run_tool('gate', '--why', 'docs only')[0].returncode, 0)
        before = git(self.origin, 'rev-parse', 'main')
        result, logged = self.run_tool('land', env={'FAIL_SCOPE': '1'})
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn('scope exit 137; MemoryMax=2560M, RuntimeMaxSec=300', result.stdout)
        self.assertIn('refusing to push', result.stdout)
        self.assertEqual(git(self.origin, 'rev-parse', 'main'), before)
        self.assertEqual((self.root / 'docs/api.md').read_text(), 'index\n')
        self.assertFalse(any(c.startswith('hirsel ') for c in logged), logged)
        self.assertTrue(any(c.startswith('systemd-run --user --scope') for c in logged), logged)

    def test_land_regenerates_after_a_push_race_without_regating_or_extra_index_commits(self):
        commit(self.root, {'README.md': 'Hirsel, a language\n'}, 'Readme')
        self.assertEqual(self.run_tool('gate', '--why', 'scratch change')[0].returncode, 0)
        records = gating.green_records(self.root)
        commit(self.sibling, {'LICENSE': 'MIT\n'}, 'Main moves during generation')
        result, logged = self.run_tool('land', env={
            'INDEX_TEXT': 'a new export\n', 'INDEX_PUSH_ONCE': str(self.sibling),
            'INDEX_PUSH_MARKER': str(self.sibling.parent / 'pushed'),
            'ASSERT_LAND_LOCK': '1',
        })
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn('main moved during the checks', result.stdout)
        self.assertEqual(git(self.origin, 'show', 'main:docs/api.md'), 'a new export')
        self.assertEqual(git(self.origin, 'show', 'main:LICENSE'), 'MIT')
        self.assertEqual(git(self.root, 'log', '--format=%s').splitlines().count('Regenerate the API index.'), 1)
        self.assertEqual(gating.green_records(self.root), records)
        self.assertEqual(logged.count('hirsel doc stdlib/std@1/hirsel.json'), 2)
        self.assertEqual(result.stdout.count('took land lock'), 1)
        self.assertEqual(result.stdout.count('held land lock for'), 1)
        self.assertFalse(any(c.startswith('test') for c in logged))

    def test_an_expired_clippy_output_is_fetched_again(self):
        commit(self.root, {'README.md': 'Hirsel, a language\n'}, 'Readme')
        self.assertEqual(self.run_tool('gate', '--why', 'scratch change')[0].returncode, 0)
        self.env['EXPIRED_ONCE'] = str(self.root.parent / 'expired')
        result, logged = self.run_tool('land')
        self.assertEqual(result.returncode, 0, result.stdout)
        self.assertEqual(len([c for c in logged if 'show-full-json-output' in c]), 2)

    def test_land_refuses_a_red_tree_and_stops_on_a_conflict(self):
        commit(self.root, {'README.md': 'Hirsel, a language\n'}, 'Readme')
        self.assertEqual(self.run_tool('gate', '--why', 'scratch change')[0].returncode, 0)
        self.env['FAIL_FMT'] = '1'
        result, _ = self.run_tool('land')
        self.assertEqual(result.returncode, 1)
        self.assertIn('land: FAIL  formatting', result.stdout)
        self.assertIn('refusing to push a red tree', result.stdout)
        self.env['FAIL_FMT'] = '0'
        self.env['FAIL_BUILD'] = '1'
        self.assertIn('FAIL  workspace build and Clippy', self.run_tool('land')[0].stdout)
        del self.env['FAIL_BUILD']
        self.sibling_pushes({'README.md': 'Hirsel, the agent language\n'})
        head = git(self.root, 'rev-parse', 'HEAD')
        result, _ = self.run_tool('land')
        self.assertEqual(result.returncode, 1)
        self.assertIn('conflicts in: README.md', result.stdout)
        self.assertEqual(git(self.root, 'rev-parse', 'HEAD'), head)
        self.assertEqual(git(self.root, 'status', '--porcelain'), '')

    def lane(self, name, files):
        """Another lane's worktree of this clone with one gated docs commit."""
        tree = self.root.parent / name
        git(self.root, 'worktree', 'add', '-q', '-b', name, str(tree), 'origin/main')
        (tree / '.buck2').mkdir()
        (tree / '.buck2/bin').symlink_to(self.root / '.buck2/bin')
        commit(tree, files, f'{name} change')
        gate = subprocess.run([str(tree / 'tools/gate'), '--why', 'docs only'], cwd=tree, env=self.env,
                              capture_output=True, text=True)
        self.assertEqual(gate.returncode, 0, gate.stdout + gate.stderr)
        return tree

    def queued_lands(self, trees):
        """Queue each tree's land behind a held lock, in order; release it; return
        each land's (exit status, output)."""
        directory = Path(self.env['HIRSEL_LAND_LOCK_DIR'])
        self.commands.unlink(missing_ok=True)
        processes = []
        with gating.fifo_slot(directory, 1, label='land', exclusive=True), contextlib.redirect_stdout(io.StringIO()):
            for count, tree in enumerate(trees, 1):
                processes.append(subprocess.Popen([str(tree / 'tools/land')], cwd=tree, env=self.env,
                                                  stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True))
                for _ in range(200):
                    if len(list((directory / 'requests').glob('*.json'))) == count:
                        break
                    gating.time.sleep(0.05)
                else:
                    self.fail(f'land {count} never queued')
        return [(process.returncode, out) for process in processes
                for out, _ in [process.communicate(timeout=120)]]

    def landings(self):
        return [json.loads(line) for line in (self.root / '.git/hirsel-landings.jsonl').read_text().splitlines()]

    def test_queued_changes_land_as_one_batch_with_one_build_and_one_push(self):
        commit(self.root, {'README.md': 'Hirsel, a language\n'}, 'Readme')
        self.assertEqual(self.run_tool('gate', '--why', 'docs only')[0].returncode, 0)
        other = self.lane('grammar', {'docs/grammar.md': 'the grammar\n'})
        before = git(self.origin, 'rev-parse', 'main')
        (first, first_out), (second, second_out) = self.queued_lands([self.root, other])
        self.assertEqual((first, second), (0, 0), first_out + second_out)
        self.assertIn('landing a batch of 2', first_out)
        self.assertIn('landed', second_out)
        self.assertIn('(batch of 2: main, grammar)', second_out)
        logged = self.commands.read_text().splitlines()
        self.assertEqual(sum(row.startswith('build ') and '--materializations none' in row for row in logged), 1, logged)
        self.assertEqual(git(self.origin, 'log', '--format=%s', f'{before}..main').splitlines(),
                         ['grammar change', 'Readme'])
        for tree in (self.root, other):
            self.assertEqual(git(tree, 'rev-parse', 'HEAD'), git(self.origin, 'rev-parse', 'main'))
        rows = self.landings()
        self.assertEqual([(row['branch'], row['batch'], row['batch_position']) for row in rows],
                         [('grammar', ['main', 'grammar'], 2), ('main', ['main', 'grammar'], 1)]
                         if rows[0]['branch'] == 'grammar' else
                         [('main', ['main', 'grammar'], 1), ('grammar', ['main', 'grammar'], 2)])

    def test_a_change_that_turns_the_batch_red_leaves_it_and_the_rest_lands(self):
        commit(self.root, {'README.md': 'Hirsel, a language\n'}, 'Readme')
        self.assertEqual(self.run_tool('gate', '--why', 'docs only')[0].returncode, 0)
        red = self.lane('poison', {'docs/poison.md': 'breaks the build\n'})
        last = self.lane('grammar', {'docs/grammar.md': 'the grammar\n'})
        before = git(self.origin, 'rev-parse', 'main')
        self.env['FAIL_BUILD_IF'] = 'docs/poison.md'
        results = self.queued_lands([self.root, red, last])
        self.assertEqual([status for status, _ in results], [0, 1, 0], ''.join(out for _, out in results))
        self.assertIn('poison turns the batch red; it leaves the batch', results[0][1])
        self.assertIn('error: docs/poison.md breaks the build', results[1][1])
        self.assertIn('this change turns the checks red on top of main', results[1][1])
        self.assertEqual(git(self.origin, 'log', '--format=%s', f'{before}..main').splitlines(),
                         ['grammar change', 'Readme'])
        # Bisected: the whole batch, the first change, the first two; then the rest in a new round.
        logged = self.commands.read_text().splitlines()
        self.assertEqual(sum(row.startswith('build ') and '--materializations none' in row for row in logged), 4, logged)
        self.assertNotEqual(git(red, 'rev-parse', 'HEAD'), git(self.origin, 'rev-parse', 'main'))

    def test_append_only_and_generated_conflicts_resolve_and_others_stop_their_change(self):
        self.sibling_pushes({'third-party/NOTICES.md': '| Source | License |\n|---|---|\n| zig | MIT |\n',
                             'docs/api.md': 'index v1\n'})
        git(self.root, 'pull', '-q', '--rebase', 'origin', 'main')
        commit(self.root, {'third-party/NOTICES.md': '| Source | License |\n|---|---|\n| zig | MIT |\n| go | BSD |\n',
                           'docs/api.md': 'index from the first lane\n', 'README.md': 'Hirsel, a language\n'},
               'First')
        self.assertEqual(self.run_tool('gate', '--why', 'docs only')[0].returncode, 0)
        rows = self.lane('rows', {'third-party/NOTICES.md': '| Source | License |\n|---|---|\n| zig | MIT |\n'
                                                            '| roc | UPL |\n',
                                  'docs/api.md': 'index from the second lane\n'})
        clash = self.lane('clash', {'README.md': 'Hirsel, the agent language\n'})
        results = self.queued_lands([self.root, rows, clash])
        self.assertEqual([status for status, _ in results], [0, 0, 1], ''.join(out for _, out in results))
        self.assertIn('resolved on the rebase: docs/api.md, third-party/NOTICES.md', results[1][1])
        self.assertIn('conflicts in: README.md', results[2][1])
        self.assertEqual(git(self.origin, 'show', 'main:third-party/NOTICES.md'),
                         '| Source | License |\n|---|---|\n| zig | MIT |\n| go | BSD |\n| roc | UPL |')
        # The index is main's side, then regenerated from the landed tree.
        self.assertEqual(git(self.origin, 'log', '-1', '--format=%s', 'main'), 'Regenerate the API index.')
        self.assertEqual(git(clash, 'status', '--porcelain'), '')

    def test_the_chosen_corpus_cases_run_first_and_their_failure_stops_the_gate(self):
        # Enough other cases that one changed case is a slice, not the whole campaign.
        self.sibling_pushes({f'conformance/LANG/LANG-{n}-y-run.hir': '// expect: run\n' for n in (2, 3, 4)})
        git(self.root, 'pull', '-q', '--rebase', 'origin', 'main')
        commit(self.root, {'conformance/LANG/LANG-1-x-run.hir': '// expect: run\n',
                           'crates/hirsel-span/tests/it/main.rs': '#[test]\nfn t() {}\n'}, 'Case')
        result, logged = self.run_tool('gate', '--why', 'a case and a test',
                                       env={'FAIL_TARGET': 'root//:gate-conformance-selected-shard-1'})
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        [tested] = [c for c in logged if c.startswith('test ')]
        self.assertIn('root//:gate-conformance-selected-shard-1 ', tested)
        self.assertNotIn('root//:a ', tested)
        self.assertIn('1 targets not run: the chosen corpus cases failed first', result.stdout)
        result, logged = self.run_tool('gate', '--extend', '--why', 'the case fixed')
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual([c.split(' -- ')[0].split()[-1] for c in logged if c.startswith('test ')],
                         ['root//:gate-conformance-selected-shard-1', 'root//:a'])

    def test_known_failures_on_main_are_excluded_and_printed_and_others_fail(self):
        self.sibling_pushes({'tools/known-failures.tsv': '# id\treason\towner\tdate\n'
                             'a\tthe replay kill-order flake\tnative-bugs-4\t2026-10-19\n'})
        git(self.root, 'fetch', '-q', 'origin')
        commit(self.root, {'crates/hirsel-span/tests/it/main.rs': '#[test]\nfn t() {}\n'}, 'Test')
        result, _ = self.run_tool('gate', '--why', 'one test target', env={'FAIL_TARGET': 'root//:a'})
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn('pass  a (FAIL, known failures only)', result.stdout)
        self.assertIn('a: known failure a: the replay kill-order flake (owner native-bugs-4, since 2026-10-19)',
                      result.stdout)
        commit(self.root, {'crates/hirsel-span/tests/it/main.rs': '#[test]\nfn u() {}\n'}, 'Other')
        result, _ = self.run_tool('gate', '--why', 'one test target', env={'FAIL_TESTS': '1'})
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.sibling_pushes({'tools/known-failures.tsv': '# id\treason\towner\tdate\n'})
        git(self.root, 'fetch', '-q', 'origin')
        commit(self.root, {'crates/hirsel-span/tests/it/main.rs': '#[test]\nfn v() {}\n'}, 'Another')
        result, _ = self.run_tool('gate', '--why', 'the row expired', env={'FAIL_TARGET': 'root//:a'})
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn('FAIL  a (FAIL', result.stdout)

    def test_land_refuses_added_known_failures_and_expires_the_landing_lanes_rows(self):
        rows = '# id\treason\towner\tdate\nx\tflaky\tmain\t2026-10-19\ny\tred\tother\t2026-10-19\n'
        self.sibling_pushes({'tools/known-failures.tsv': rows})
        git(self.root, 'pull', '-q', '--rebase', 'origin', 'main')
        commit(self.root, {'tools/known-failures.tsv': rows + 'z\tmine\tmain\t2026-10-20\n',
                           'bench/native/tests/test_fixture.py':
                               'import unittest\nclass Fixture(unittest.TestCase):\n    def test_available(self): pass\n'},
               'Add a row')
        gate = self.run_tool('gate', '--why', 'tooling')[0]
        self.assertEqual(gate.returncode, 0, gate.stdout + gate.stderr)
        result, _ = self.run_tool('land')
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn('the change adds rows to tools/known-failures.tsv: z', result.stdout)
        git(self.root, 'reset', '-q', '--hard', 'origin/main')
        commit(self.root, {'README.md': 'Hirsel, a language\n'}, 'Readme')
        self.assertEqual(self.run_tool('gate', '--why', 'docs only')[0].returncode, 0)
        result, _ = self.run_tool('land')
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn('expired known failures owned by the landing lanes: x', result.stdout)
        self.assertEqual(git(self.origin, 'show', 'main:tools/known-failures.tsv'),
                         '# id\treason\towner\tdate\ny\tred\tother\t2026-10-19')
        self.assertIn('Expire the known failures of main.', git(self.origin, 'log', '--format=%s', '-3', 'main'))

    def test_a_restarted_gate_keeps_its_place_in_the_queue(self):
        commit(self.root, {'crates/hirsel-span/tests/it/main.rs': '#[test]\nfn t() {}\n'}, 'Test')
        directory = self.root.parent / 'slots'
        env = {gating.GATE_SLOTS: str(directory)}
        queue = directory / 'express-queue'
        with contextlib.ExitStack() as held:
            for _ in range(2):
                held.enter_context(gating.fifo_slot(directory, 2, pool=gating.EXPRESS_POOL))
            with self.daemon_process(['gate', '--why', 'one test target'], env) as process:
                self.wait_for_ticket(queue, 1)
                [first] = list(queue.iterdir())
                process.terminate()
                process.communicate(timeout=20)
            # A later waiter queues; the restarted gate goes back ahead of it.
            later = queue / f'{time_ns()}-{os.getpid()}'
            later.write_text(f'{gating.process_start(os.getpid())}\n')
            with self.daemon_process(['gate', '--why', 'one test target'], env) as process:
                self.wait_for_ticket(queue, 2)
                [restarted] = [ticket for ticket in queue.iterdir() if ticket != later]
                self.assertEqual(restarted.name.split('-')[0], first.name.split('-')[0])
                self.assertEqual(gating.oldest_gate_ticket(queue), restarted)
                later.unlink()
                held.close()
                out, err = process.communicate(timeout=30)
                self.assertEqual(process.returncode, 0, out + err)
                self.assertIn("keeping this worktree's place in the queue", out)
        self.assertFalse((self.root / '.git/hirsel-gate-queue.json').exists())
        [entry] = [json.loads(line) for line in (self.root / '.git/hirsel-gates.jsonl').read_text().splitlines()][-1:]
        self.assertTrue(entry['passed'])
        self.assertIsNotNone(entry['floor_seconds'])

    def wait_for_ticket(self, queue, count):
        for _ in range(400):
            if queue.is_dir() and len(list(queue.iterdir())) == count:
                return
            gating.time.sleep(0.05)
        self.fail(f'{count} tickets never queued')

    def test_a_crate_edit_checks_that_every_ignored_test_runs_in_a_gate(self):
        commit(self.root, {'crates/hirsel-span/tests/it/main.rs': '#[test]\n#[ignore]\nfn tool() {}\n'}, 'Test')
        result, logged = self.run_tool('gate', '--why', 'scratch change', env={'FAIL_IGNORED': '1'})
        self.assertIn('ignored_tests.py', logged)
        self.assertIn('FAIL  tools/tests/ignored_tests.py', result.stdout)
        self.assertIn('pass  tools/tests/ignored_tests.py', self.run_tool('gate', '--why', 'scratch change')[0].stdout)

    def test_a_gated_change_rebased_cleanly_only_builds_and_checks_formatting(self):
        commit(self.root, {'docs/grammar.md': 'grammar, revised\n'}, 'Grammar')
        # docs/grammar.md is a compile input (BUCK names it): this reaches Buck2.
        identity = gating.change_identity(gating.merge_base('origin/main', self.root), self.root)
        gating.record_gate(identity, set(), self.root)
        self.sibling_pushes({'LICENSE': 'MIT\n'})
        git(self.root, 'pull', '-q', '--rebase')
        result, logged = self.run_tool('gate', '--why', 'scratch change')
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn('only the workspace build and formatting run', result.stdout)
        self.assertEqual([c for c in logged if c.startswith(('build', 'test'))],
                         ['build -c hirsel.clippy=true -c buck2.test_builds_targets=true --materializations none root//:a'])
        self.assertIn('fmt.py --check', logged)


    def test_a_gate_says_why_its_selection_suffices(self):
        commit(self.root, {'crates/hirsel-span/tests/it/main.rs': '#[test]\nfn t() {}\n'}, 'Test')
        for arguments in [(), ('--why', ' '), ('--tests', 'conformance')]:
            with self.subTest(arguments=arguments):
                result, logged = self.run_tool('gate', *arguments)
                self.assertEqual(result.returncode, 2, result.stdout)
                self.assertIn('--why', result.stderr)
                self.assertEqual(logged, [])

    def test_formatting_clippy_or_the_build_failing_stops_the_gate_before_any_test(self):
        commit(self.root, {'crates/hirsel-span/tests/it/main.rs': '#[test]\nfn t() {}\n'}, 'Test')
        for env in [{'FAIL_FMT': '1'}, {'FAIL_BUILD': '1'}]:
            with self.subTest(env=env):
                result, logged = self.run_tool('gate', '--why', 'scratch change', '--tests', 'conformance', env=env)
                self.assertEqual(result.returncode, 1, result.stdout)
                self.assertIn('no test ran', result.stdout)
                self.assertFalse(any(c.startswith('test') for c in logged), logged)
        result, logged = self.run_tool('gate', '--why', 'scratch change', '--tests', 'conformance')
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn('gate: pool: normal', result.stdout)
        [test] = [c for c in logged if c.startswith('test')]
        self.assertIn('root//:gate-conformance-shard-16', test)

    def test_suggest_runs_nothing_and_prints_the_floor_and_the_selection(self):
        commit(self.root, {'crates/hirsel-lexer/src/lib.rs': 'pub fn f() {}\n'}, 'Lexer')
        result, logged = self.run_tool('gate', '--suggest')
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn('the floor', result.stdout)
        self.assertIn('conformance: the whole campaign', result.stdout)
        self.assertIn('agreement: the whole campaign', result.stdout)
        self.assertFalse(any(c.startswith(('build', 'test')) for c in logged), logged)

    def test_a_local_failure_stops_the_gate_before_the_queue_and_land_refuses_red(self):
        commit(self.root, {'crates/hirsel-span/tests/it/main.rs': '#[test]\nfn t() {}\n'}, 'Test')
        directory = self.root.parent / 'slots'
        result, logged = self.run_tool('gate', '--tests', 'conformance', '--why', 'corpus and local checks',
                                       env={'FAIL_IGNORED': '1', gating.GATE_SLOTS: str(directory)})
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn('no slot taken and no test ran', result.stdout)
        self.assertFalse(any(c.startswith('test ') for c in logged), logged)
        self.assertFalse((directory / 'queue').exists())
        identity = gating.change_identity(gating.merge_base('origin/main', self.root), self.root)
        record = gating.green_records(self.root)[identity]
        self.assertFalse(gating.is_green(identity, set(), self.root))
        self.assertEqual(record['target_results'], {})
        self.assertEqual(gating.latest_green(self.root), (None, None))
        self.assertEqual(self.run_tool('land')[0].returncode, 1)
        result, logged = self.run_tool('gate', '--extend', '--why', 'local check fixed')
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertTrue(any(c.startswith('test ') for c in logged), logged)
        self.assertIn('ignored_tests.py', logged)
        record = gating.green_records(self.root)[identity]
        self.assertTrue(gating.is_green(identity, set(), self.root))
        self.assertIn('conformance', record['selection'])
        self.assertIn('corpus and local checks', record['why'])
        self.assertEqual(self.run_tool('land')[0].returncode, 0)

    def test_extend_reruns_failed_and_missing_targets_and_preserves_campaign_summary(self):
        commit(self.root, {'crates/hirsel-span/tests/it/main.rs': '#[test]\nfn t() {}\n'}, 'Test')
        result, _ = self.run_tool('gate', '--tests', 'conformance', '--why', 'corpus', env={
            'FAIL_TARGET': 'root//:gate-conformance-shard-3',
            'OMIT_TARGET': 'root//:gate-conformance-shard-7',
        })
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        result, logged = self.run_tool('gate', '--extend', '--why', 'retry failures')
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        [test] = [c for c in logged if c.startswith('test ')]
        self.assertEqual([word for word in test.split() if word.startswith('root//')],
                         ['root//:gate-conformance-shard-3', 'root//:gate-conformance-shard-7'])
        self.assertIn('16 validated / 16 passing', result.stdout)

    def test_partial_results_never_reuse_a_changed_source(self):
        commit(self.root, {'crates/hirsel-span/tests/it/main.rs': '#[test]\nfn t() {}\n'}, 'Test')
        result, _ = self.run_tool('gate', '--tests', 'conformance', '--why', 'corpus',
                                  env={'FAIL_IGNORED': '1'})
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        commit(self.root, {'crates/hirsel-span/tests/it/main.rs': '#[test]\nfn changed() {}\n'}, 'Edit')
        result, logged = self.run_tool('gate', '--tests', 'conformance', '--why', 'changed source')
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        [test] = [c for c in logged if c.startswith('test ')]
        self.assertIn('root//:gate-conformance-shard-16', test)
        self.assertIn('root//:a', test)

    def test_partial_results_never_reuse_changed_campaign_settings(self):
        commit(self.root, {'crates/hirsel-span/tests/it/main.rs': '#[test]\nfn t() {}\n'}, 'Test')
        result, _ = self.run_tool('gate', '--tests', 'native:README.md', '--why', 'first program',
                                  env={'FAIL_IGNORED': '1'})
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        result, logged = self.run_tool('gate', '--extend', '--tests', 'native:docs/grammar.md',
                                       '--why', 'second program')
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        [test] = [c for c in logged if c.startswith('test ')]
        self.assertIn('root//:gate-native-changed', test)
        self.assertIn('root//:gate-native-cranelift-changed', test)
        self.assertNotIn('reusing', result.stdout)

    def test_a_build_failure_during_resume_keeps_passed_target_results(self):
        commit(self.root, {'crates/hirsel-span/tests/it/main.rs': '#[test]\nfn t() {}\n'}, 'Test')
        result, _ = self.run_tool('gate', '--tests', 'conformance', '--why', 'corpus',
                                  env={'FAIL_TARGET': 'root//:gate-conformance-shard-3'})
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        result, logged = self.run_tool('gate', '--extend', '--why', 'build failed', env={'FAIL_BUILD': '1'})
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertFalse(any(c.startswith('test ') for c in logged), logged)
        result, logged = self.run_tool('gate', '--extend', '--why', 'build fixed')
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        [tested] = [c for c in logged if c.startswith('test ')]
        self.assertIn('root//:gate-conformance-shard-3 ', tested)
        self.assertNotIn('root//:gate-conformance-shard-4 ', tested)

    def test_resuming_a_failed_extension_keeps_its_scope_and_inherited_selection(self):
        commit(self.root, {'crates/hirsel-span/tests/it/main.rs': '#[test]\nfn t() {}\n'}, 'Test')
        result, _ = self.run_tool('gate', '--tests', 'conformance', '--why', 'initial selection')
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        changed = 'crates/hirsel-lexer/tests/it/keywords.rs'
        commit(self.root, {changed: '#[test]\nfn k() {}\n'}, 'Edit')
        result, _ = self.run_tool('gate', '--extend', '--why', 'extension', env={'FAIL_IGNORED': '1'})
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        result, logged = self.run_tool('gate', '--extend', '--why', 'local fixed')
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        # The failed extension stopped in its floor; its resumption runs its tests.
        self.assertTrue(any(c.startswith('test ') for c in logged), logged)
        _, record = gating.latest_green(self.root)
        self.assertEqual(record['paths'], [changed])
        self.assertIn('conformance', record['selection'])
        self.assertIsNotNone(record['extends'])

    def test_extend_runs_the_edits_floor_and_land_accepts_the_new_identity(self):
        result, _ = self.run_tool('gate', '--extend', '--why', 'scratch change')
        self.assertEqual(result.returncode, 0)
        commit(self.root, {'crates/hirsel-span/tests/it/main.rs': '#[test]\nfn t() {}\n'}, 'Test')
        result, _ = self.run_tool('gate', '--extend', '--why', 'scratch change')
        self.assertEqual(result.returncode, 1)
        self.assertIn('--extend needs a change this worktree passed', result.stdout)
        result, _ = self.run_tool('gate', '--why', 'the span test alone', '--tests', 'conformance')
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        first = gating.change_identity(gating.merge_base('origin/main', self.root), self.root)
        commit(self.root, {'crates/hirsel-lexer/tests/it/keywords.rs': '#[test]\nfn k() {}\n'}, 'Fix')
        result, logged = self.run_tool('gate', '--extend', '--why', 'one more test file')
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn(f'extending change {first[:12]}', result.stdout)
        self.assertIn('1 paths differ: crates/hirsel-lexer/tests/it/keywords.rs', result.stdout)
        [test] = [c for c in logged if c.startswith('test')]
        self.assertNotIn('gate-conformance-shard', test)
        identity = gating.change_identity(gating.merge_base('origin/main', self.root), self.root)
        record = gating.green_records(self.root)[identity]
        self.assertEqual((record['extends'], record['why']),
                         (first, 'one more test file (extending: the span test alone)'))
        self.assertIn('conformance', record['selection'])
        self.assertEqual(record['pool'], 'express')
        result, _ = self.run_tool('land')
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn('land: gated ', result.stdout)
        self.assertIn('why: one more test file (extending: the span test alone)', result.stdout)
        self.assertIn(f'extends change {first[:12]}', result.stdout)
        [landing] = (Path(git(self.root, 'rev-parse', '--path-format=absolute', '--git-common-dir'))
                     / 'hirsel-landings.jsonl').read_text().splitlines()
        landing = json.loads(landing)
        self.assertEqual((landing['pushed'], landing['identity'], landing['why'], landing['extends']),
                         (git(self.origin, 'rev-parse', 'main'), identity,
                          'one more test file (extending: the span test alone)', first))

if __name__ == '__main__':
    unittest.main()
