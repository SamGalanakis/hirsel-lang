#!/usr/bin/env python3
"""tools/gate's selection, change identity and summary, and tools/land's loop,
against scratch repositories with a fake Buck2."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest

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
  build) [[ $* == *show-full-json-output* ]] && echo '{}'; exit "${FAIL_BUILD:-0}" ;;
esac
'''
FAKE_SCRIPT = '''import os, sys
CONFORMANCE_SHARDS = 16
AGREE_SHARDS = 16
AGREE_LIMIT = 5000
AGREE_FULL_SHARDS = 96
AGREE_FULL_TIMEOUT = 1800
if __name__ == "__main__":
    with open(os.environ["COMMANDS"], "a") as log:
        print(" ".join(["{name}", *sys.argv[1:]]), file=log)
    sys.exit(int(os.environ.get("FAIL_{upper}", "0")))
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
                 **{f'crates/{crate}/Cargo.toml': f'[package]\nname = "{crate}"\n' for crate in impact.CRATE_CAMPAIGNS},
                 '.gitignore': (REPO / '.gitignore').read_text()}
        for name in ('gate', 'land'):
            files[f'tools/{name}'] = (TOOLS / name).read_text()
        for name in ('gating', 'impact', 'clippy', 'agree', 'conformance'):
            files[f'tools/buck2/{name}.py'] = (TOOLS / 'buck2' / f'{name}.py').read_text()
        for name in ('sync', 'prepare', 'fmt'):
            files[f'tools/buck2/{name}.py'] = FAKE_SCRIPT.replace('{name}', f'{name}.py').replace('{upper}', name.upper())
        commit(self.root, files, 'Base')
        for name in ('gate', 'land'):
            (self.root / 'tools' / name).chmod(0o755)
        git(self.root, 'commit', '-q', '-am', 'Executable tools')
        buck2 = self.root / '.buck2/bin/buck2'
        buck2.parent.mkdir(parents=True)
        buck2.write_text(FAKE_BUCK2)
        buck2.chmod(0o755)
        subprocess.run(['git', 'clone', '-q', '--bare', str(self.root), str(self.origin)], check=True)
        git(self.root, 'remote', 'add', 'origin', str(self.origin))
        git(self.root, 'fetch', '-q', 'origin')
        git(self.root, 'branch', '-q', '--set-upstream-to=origin/main')
        self.commands = top / 'commands'
        self.env = os.environ | {'COMMANDS': str(self.commands)}
        # A sibling's clone, to move main.
        self.sibling = top / 'sibling'
        subprocess.run(['git', 'clone', '-q', str(self.origin), str(self.sibling)], check=True)
        git(self.sibling, 'config', 'user.email', 'test@example.invalid')
        git(self.sibling, 'config', 'user.name', 'Sibling')

    def run_tool(self, *command):
        self.commands.unlink(missing_ok=True)
        result = subprocess.run([str(self.root / 'tools' / command[0]), *command[1:]], cwd=self.root,
                                env=self.env, capture_output=True, text=True)
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
