#!/usr/bin/env python3
"""tools/gate's selection, change identity and summary, and tools/land's loop,
against scratch repositories with a fake Buck2."""
import contextlib
import io
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import threading
import unittest
from unittest.mock import patch

sys.dont_write_bytecode = True
TOOLS = Path(__file__).resolve().parents[1]
REPO = TOOLS.parent
sys.path.insert(0, str(TOOLS / 'buck2'))
import gating  # noqa: E402
import impact  # noqa: E402

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
        self.assertEqual(gating.macro_targets({'a-test-it'}),
                         'attrregexfilter(name, "^(a\\-test\\-it)(-harness|-host|-shard-[0-9]+)?$", root//:)')

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

    def test_a_deleted_file_stands_for_its_directorys_files(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / 'crates/a/src').mkdir(parents=True)
            for path in ['crates/a/src/lib.rs', 'crates/a/src/kept.rs', 'crates/a/Cargo.toml']:
                (root / path).write_text('')
            self.assertEqual(
                gating.owned_paths(['crates/a/src/gone.rs', 'crates/a/Cargo.toml'], root),
                ['crates/a/Cargo.toml', 'crates/a/src/kept.rs', 'crates/a/src/lib.rs'])
            self.assertEqual(gating.owned_paths(['crates/b/src/gone/deep.rs'], root), [])

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


class Queries(unittest.TestCase):
    """The Buck2 queries a selection asks, with uquery recorded."""

    def resolve(self, paths, owners_with_gate=(), buck=False):
        asked = []

        def uquery(expression):
            asked.append(expression)
            if expression.startswith('attrfilter(labels, gate,'):
                return list(owners_with_gate)
            return ['root//:t-unittest']
        selection = gating.Selection.__new__(gating.Selection)
        selection.paths = paths
        selection.classified = impact.classify(paths, MEMBERS, BUCK)
        if buck:
            selection.classified.buck = buck
        selection.tests, selection.campaigns = [], set()
        original = gating.uquery
        gating.uquery = uquery
        try:
            selection.resolve()
        finally:
            gating.uquery = original
        return selection, asked

    def test_an_input_selects_the_tests_reaching_it_and_the_campaigns_reading_it(self):
        selection, asked = self.resolve(['conformance/passing.txt'], ['root//:gate-conformance-shard-3', 'root//:gate-native-smoke'])
        self.assertEqual(selection.campaigns, {impact.CONFORMANCE, impact.NATIVE_SMOKE})
        self.assertIn('(kind("_rust_test", rdeps(//..., owner("conformance/passing.txt"))) + attrfilter(labels, doctest, '
                      'rdeps(//..., owner("conformance/passing.txt")))) - attrfilter(labels, gate, //...)', asked)
        self.assertEqual(selection.tests, ['root//:t-unittest'])

    def test_a_library_edit_adds_its_crates_campaigns_and_a_buck_edit_its_targets(self):
        selection, asked = self.resolve(['crates/hirsel-lexer/src/lib.rs'], buck={'hirsel-llvm-test-it'})
        self.assertEqual(selection.campaigns, {impact.CONFORMANCE, impact.AGREEMENT})
        self.assertIn(gating.macro_targets({'hirsel-llvm-test-it'}), asked[-1])
        self.assertEqual(len(selection.campaign_targets()), 32)

    def test_a_build_wide_edit_selects_every_test_and_campaign(self):
        selection, asked = self.resolve(['.buckconfig'])
        self.assertEqual(asked, [gating.ALL_TESTS])
        self.assertEqual(selection.campaigns, set(impact.CAMPAIGNS))


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

    def test_filtered_targets_share_the_command_and_cache_selection(self):
        selection = gating.Selection.__new__(gating.Selection)
        selection.paths = ['conformance/MEM/soa.hir']
        selection.classified = impact.Classified(crates={'hirsel-llvm'})
        selection.campaigns = set()
        with patch.object(gating, 'ROOT', self.root):
            self.assertEqual(selection.native_targets(),
                             ['root//:gate-native-changed', 'root//:gate-native-cranelift-changed'])
            self.assertEqual(selection.campaign_targets(), selection.native_targets())


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

    def test_no_change_has_no_identity(self):
        git(self.root, 'reset', '-q', '--hard', 'main')
        self.assertEqual(self.identity(), '')

    def test_a_green_record_names_the_identity_and_its_crates(self):
        identity = self.identity()
        self.assertFalse(gating.is_green(identity, {'hirsel-lexer'}, self.root))
        gating.record_green(identity, {'hirsel-lexer'}, self.root)
        self.assertTrue(gating.is_green(identity, {'hirsel-lexer'}, self.root))
        self.assertFalse(gating.is_green(identity, {'hirsel-llvm'}, self.root))
        self.assertFalse(gating.is_green('', set(), self.root))


class Slots(unittest.TestCase):
    def test_a_gate_waits_while_every_slot_is_held_and_takes_the_first_freed(self):
        with tempfile.TemporaryDirectory() as scratch:
            environ = {gating.GATE_SLOTS: f'{scratch}/slots:2'}
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
            self.assertIn('waiting for one of 2 gate slots', said.getvalue())
            self.assertIn('took gate slot', said.getvalue())

    def test_the_earlier_waiter_acquires_before_a_later_waiter_that_retries_first(self):
        with tempfile.TemporaryDirectory() as scratch:
            environ = {gating.GATE_SLOTS: f'{scratch}/slots:1'}
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
                    with gating.gate_slot(environ):
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
                    with gating.gate_slot(environ):
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
                self.assertEqual(list((Path(scratch) / 'slots/queue').iterdir()), [])

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
            environ = {gating.GATE_SLOTS: f'{scratch}/slots:1'}
            queue = Path(scratch) / 'slots/queue'

            def interrupt(_):
                self.assertEqual(len(list(queue.iterdir())), 1)
                raise KeyboardInterrupt

            with gating.gate_slot(environ), patch.object(gating.time, 'sleep', interrupt):
                with self.assertRaises(KeyboardInterrupt), contextlib.redirect_stdout(io.StringIO()):
                    with gating.gate_slot(environ):
                        self.fail('a waiter entered an occupied slot')
                self.assertEqual(list(queue.iterdir()), [])

    def test_no_cap_without_the_variable_and_a_malformed_one_is_refused(self):
        with gating.gate_slot({}), gating.gate_slot({}), gating.gate_slot({}):
            pass
        with self.assertRaises(SystemExit), gating.gate_slot({gating.GATE_SLOTS: '/tmp/slots'}):
            pass


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
            '.buck2/bin/buck2 test //:gate-conformance-shard-6'])
        agreement = json.dumps({'checked': 3, 'clusters': [{'kind': 'source accepts', 'path': 'conformance/x.hir'}]})
        lines = gating.rerun('gate-agree-shard-2', agreement)
        self.assertEqual(lines[0], 'source accepts: conformance/x.hir')
        self.assertIn('--program conformance/x.hir', lines[1])
        self.assertEqual(lines[-1], '.buck2/bin/buck2 test //:gate-agree-shard-2')


FAKE_BUCK2 = '''#!/usr/bin/env bash
printf '%s\\n' "$*" >> "$COMMANDS"
case $1 in
  uquery) [[ $* == *'kind("^rust_'* ]] && echo 'root//:a-clippy' || echo 'root//:a'; exit 0 ;;
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
if __name__ == "__main__":
    with open(os.environ["COMMANDS"], "a") as log:
        print(" ".join(["{name}", *sys.argv[1:]]), file=log)
    sys.exit(int(os.environ.get("FAIL_{upper}", "0")))
'''

FAKE_HIRSEL = '''#!/usr/bin/env python3
import os
from pathlib import Path
import subprocess
import sys
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
        files = {'Cargo.toml': '[workspace]\nmembers = ["crates/*"]\n', 'BUCK': BUCK, 'README.md': 'Hirsel\n',
                 'docs/building.md': 'See [the grammar](grammar.md).\n', 'docs/grammar.md': 'grammar\n',
                 'docs/api.md': 'index\n', 'docs/catalogue.md': 'index\n',
                 **{f'crates/{crate}/Cargo.toml': f'[package]\nname = "{crate}"\n' for crate in impact.CRATE_CAMPAIGNS},
                 '.gitignore': (REPO / '.gitignore').read_text()}
        for name in ('gate', 'land'):
            files[f'tools/{name}'] = (TOOLS / name).read_text()
        for name in ('gating', 'impact', 'clippy', 'agree', 'conformance'):
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
        buck2.write_text(FAKE_BUCK2)
        buck2.chmod(0o755)
        hirsel = buck2.with_name('hirsel')
        hirsel.write_text(FAKE_HIRSEL)
        hirsel.chmod(0o755)
        subprocess.run(['git', 'clone', '-q', '--bare', str(self.root), str(self.origin)], check=True)
        git(self.root, 'remote', 'add', 'origin', str(self.origin))
        git(self.root, 'fetch', '-q', 'origin')
        git(self.root, 'branch', '-q', '--set-upstream-to=origin/main')
        self.commands = top / 'commands'
        self.env = os.environ | {'COMMANDS': str(self.commands)}
        # Scratch gates must not acquire a slot held by the gate running this test.
        self.env.pop('HIRSEL_GATE_SLOTS', None)
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

    def sibling_pushes(self, files):
        commit(self.sibling, files, 'Sibling change')
        git(self.sibling, 'push', '-q', 'origin', 'HEAD:main')

    def test_documentation_alone_runs_no_buck2_and_catches_broken_links(self):
        commit(self.root, {'docs/building.md': 'See [the grammar](grammar.md) and [x](gone.md).\n'}, 'Docs')
        result, logged = self.run_tool('gate')
        self.assertEqual(result.returncode, 1, result.stdout)
        self.assertIn('docs/building.md: link gone.md names nothing', result.stdout)
        self.assertEqual(logged, [])
        commit(self.root, {'docs/building.md': 'See [the grammar](grammar.md).\nMore.\n'}, 'Docs fixed')
        result, logged = self.run_tool('gate')
        self.assertEqual(result.returncode, 0, result.stdout)
        self.assertEqual(logged, [])
        # Moving a linked document without its links fails.
        commit(self.root, {'docs/grammar.md': None, 'docs/syntax.md': 'grammar\n'}, 'Move')
        result, _ = self.run_tool('gate')
        self.assertIn('docs/building.md: link grammar.md names a deleted path', result.stdout)

    def test_land_refuses_an_ungated_change_and_lands_a_gated_one_after_main_moves(self):
        commit(self.root, {'README.md': 'Hirsel, a language\n'}, 'Readme')
        result, logged = self.run_tool('land')
        self.assertEqual(result.returncode, 1)
        self.assertIn('tools/gate has not passed this change', result.stdout)
        self.assertFalse(any(c.startswith('build') for c in logged))
        self.assertEqual(self.run_tool('gate')[0].returncode, 0)
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

    def test_land_commits_the_index_generated_from_the_rebased_sources(self):
        commit(self.root, {'README.md': 'Hirsel, a language\n'}, 'Readme')
        self.assertEqual(self.run_tool('gate')[0].returncode, 0)
        self.sibling_pushes({'docs/catalogue.md': 'a new export\n'})
        records = gating.green_records(self.root)
        result, logged = self.run_tool('land')
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(git(self.origin, 'show', 'main:docs/api.md'), 'a new export')
        self.assertEqual(git(self.root, 'log', '-1', '--format=%s'), 'Regenerate the API index.')
        self.assertEqual(git(self.root, 'diff-tree', '--no-commit-id', '--name-only', '-r', 'HEAD'), 'docs/api.md')
        self.assertEqual(gating.green_records(self.root), records)
        self.assertIn('hirsel doc --index', logged)
        self.assertFalse(any(c.startswith('test') for c in logged))

    def test_land_does_not_push_or_replace_the_page_when_index_generation_fails(self):
        commit(self.root, {'README.md': 'Hirsel, a language\n'}, 'Readme')
        self.assertEqual(self.run_tool('gate')[0].returncode, 0)
        before = git(self.origin, 'rev-parse', 'main')
        result, _ = self.run_tool('land', env={'FAIL_INDEX': '1'})
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn('index generation failed', result.stdout)
        self.assertEqual(git(self.origin, 'rev-parse', 'main'), before)
        self.assertEqual((self.root / 'docs/api.md').read_text(), 'index\n')
        self.assertEqual(git(self.root, 'status', '--porcelain'), '')

    def test_land_regenerates_after_a_push_race_without_regating_or_extra_index_commits(self):
        commit(self.root, {'README.md': 'Hirsel, a language\n'}, 'Readme')
        self.assertEqual(self.run_tool('gate')[0].returncode, 0)
        records = gating.green_records(self.root)
        commit(self.sibling, {'LICENSE': 'MIT\n'}, 'Main moves during generation')
        result, logged = self.run_tool('land', env={
            'INDEX_TEXT': 'a new export\n', 'INDEX_PUSH_ONCE': str(self.sibling),
            'INDEX_PUSH_MARKER': str(self.sibling.parent / 'pushed'),
        })
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn('main moved during the checks', result.stdout)
        self.assertEqual(git(self.origin, 'show', 'main:docs/api.md'), 'a new export')
        self.assertEqual(git(self.origin, 'show', 'main:LICENSE'), 'MIT')
        self.assertEqual(git(self.root, 'log', '--format=%s').splitlines().count('Regenerate the API index.'), 1)
        self.assertEqual(gating.green_records(self.root), records)
        self.assertEqual(logged.count('hirsel doc --index'), 2)
        self.assertFalse(any(c.startswith('test') for c in logged))

    def test_an_expired_clippy_output_is_fetched_again(self):
        commit(self.root, {'README.md': 'Hirsel, a language\n'}, 'Readme')
        self.assertEqual(self.run_tool('gate')[0].returncode, 0)
        self.env['EXPIRED_ONCE'] = str(self.root.parent / 'expired')
        result, logged = self.run_tool('land')
        self.assertEqual(result.returncode, 0, result.stdout)
        self.assertEqual(len([c for c in logged if 'show-full-json-output' in c]), 2)

    def test_land_refuses_a_red_tree_and_stops_on_a_conflict(self):
        commit(self.root, {'README.md': 'Hirsel, a language\n'}, 'Readme')
        self.assertEqual(self.run_tool('gate')[0].returncode, 0)
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

    def test_a_crate_edit_checks_that_every_ignored_test_runs_in_a_gate(self):
        commit(self.root, {'crates/hirsel-span/tests/it/main.rs': '#[test]\n#[ignore]\nfn tool() {}\n'}, 'Test')
        result, logged = self.run_tool('gate', env={'FAIL_IGNORED': '1'})
        self.assertIn('ignored_tests.py', logged)
        self.assertIn('FAIL  tools/tests/ignored_tests.py', result.stdout)
        self.assertIn('pass  tools/tests/ignored_tests.py', self.run_tool('gate')[0].stdout)

    def test_a_gated_change_rebased_cleanly_only_builds_and_checks_formatting(self):
        commit(self.root, {'docs/grammar.md': 'grammar, revised\n'}, 'Grammar')
        # docs/grammar.md is a compile input (BUCK names it): this reaches Buck2.
        identity = gating.change_identity(gating.merge_base('origin/main', self.root), self.root)
        gating.record_green(identity, set(), self.root)
        self.sibling_pushes({'LICENSE': 'MIT\n'})
        git(self.root, 'pull', '-q', '--rebase')
        result, logged = self.run_tool('gate')
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn('only the workspace build and formatting run', result.stdout)
        self.assertEqual([c for c in logged if c.startswith(('build', 'test'))],
                         ['build -c hirsel.clippy=true -c buck2.test_builds_targets=true --materializations none root//:a'])
        self.assertIn('fmt.py --check', logged)


if __name__ == '__main__':
    unittest.main()
