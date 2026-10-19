#!/usr/bin/env python3
"""Caller selection, explicit inputs and shard coverage at the launcher seam."""
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
LAUNCHER = ROOT / 'tools/buck2/test_launcher.sh'
CASES = {
    'pool::full_gate_heap': True,
    'host::full_gate_arm': True,
    'pool::full_gate_prefix': True,
    'pool::full_gate_prefix_host': True,
    'host::smoke': False,
    'pool::smoke': False,
    'pool::other': False,
}
# Emulate the external libtest CLI with argparse, independently of the shell
# selector. Real Buck2 --list probes also exercise the pinned Rust harness.
HARNESS = '''#!/usr/bin/env python3
import argparse, json, os, sys
parser = argparse.ArgumentParser()
for option in ('ignored', 'include-ignored', 'exact', 'list', 'nocapture', 'show-output', 'quiet', 'report-time'):
    parser.add_argument('--' + option, action='store_true')
parser.add_argument('--skip', action='append', default=[])
for option in ('format', 'color', 'test-threads', 'logfile', 'shuffle-seed'):
    parser.add_argument('--' + option)
parser.add_argument('-Z')
parser.add_argument('filters', nargs='*')
args = parser.parse_args()
cases = json.loads(os.environ['CASES'])
match = lambda name, pattern: name == pattern if args.exact else pattern in name
selected = [name for name, ignored in cases.items()
            if (not args.filters or any(match(name, f) for f in args.filters))
            and not any(match(name, f) for f in args.skip)
            and (not args.ignored or ignored)]
if args.list:
    for name in selected: print(name + ': test')
else:
    with open(os.environ['OBSERVED'], 'w') as output:
        json.dump(dict(argv=sys.argv[1:], bootstrap=os.environ.get('RUSTC_BOOTSTRAP'), cases=[name for name in selected
                  if args.ignored or args.include_ignored or not cases[name]]), output)
    for name in selected:
        status = 'ok' if args.ignored or args.include_ignored or not cases[name] else 'ignored'
        print('test ' + name + ' ... ' + status)
'''


class LauncherFixture(unittest.TestCase):
    """A fake harness under the launcher, as a test action runs it."""

    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        (self.root / 'llvm/bin').mkdir(parents=True)
        (self.root / 'llvm/lib').mkdir()
        harness = self.root / 'harness'
        harness.write_text(HARNESS)
        harness.chmod(0o755)
        self.observed = self.root / 'observed.json'
        self.env = os.environ | dict(
            HIRSEL_TEST_TMPDIR=str(self.root / 'tmp'), HIRSEL_TEST_CWD=str(self.root),
            LLVM_SYS_211_PREFIX=str(self.root / 'llvm'), CASES=json.dumps(CASES),
            OBSERVED=str(self.observed), HIRSEL_TEST_BINARY='fixture-test-it')
        for key in ('HIRSEL_TEST_POOL', 'TEST_SHARD_INDEX', 'TEST_TOTAL_SHARDS', 'HIRSEL_TEST_HOST_ONLY'):
            self.env.pop(key, None)


class LauncherSelection(LauncherFixture):
    def run_cases(self, *args, shard=''):
        self.observed.unlink(missing_ok=True)
        env = self.env.copy()
        if shard:
            # `<i>/<n>` with i from 1, as Buck2 target names count shards;
            # the launcher reads Bazel's 0-based variables.
            index, total = shard.split('/')
            env |= {'TEST_SHARD_INDEX': str(int(index) - 1), 'TEST_TOTAL_SHARDS': total}
        run = subprocess.run(['sh', str(LAUNCHER), 'harness', *args], cwd=self.root,
                             env=env, capture_output=True, text=True)
        self.assertEqual(run.returncode, 0, run.stderr)
        result = json.loads(self.observed.read_text()) if self.observed.exists() else {'cases': [], 'argv': []}
        return result, run.stdout

    def test_host_scratch_lives_outside_the_project_and_survives_concurrent_links(self):
        # Buck2 stops at a non-UTF-8 name in the project tree; native tests make one.
        scratch = self.root / 'host-tmp'
        self.env['TMPDIR'] = str(scratch)
        (self.root / 'tmp').mkdir()
        (self.root / 'tmp/stale').write_text('from a run before the link')
        runs = [subprocess.Popen(['sh', str(LAUNCHER), 'harness', '--list'], cwd=self.root,
                                 env=self.env, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
                for _ in range(8)]
        for run in runs:
            _, stderr = run.communicate()
            self.assertEqual(run.returncode, 0, stderr)
        link = self.root / 'tmp'
        self.assertTrue(link.is_symlink())
        target = Path(os.readlink(link))
        self.assertEqual(target.parent, scratch)
        self.assertTrue(target.name.startswith('hirsel-test-'))
        self.assertEqual(list(target.iterdir()), [])

    def test_every_native_case_runs_on_the_pool_even_under_exact(self):
        for case in CASES:
            result, _ = self.run_cases(case, '--exact', '--include-ignored', shard='1/3')
            self.assertEqual(result['cases'], [case])

    def test_shards_cover_all_selected_cases_once(self):
        actual = []
        for shard in ('1/3', '2/3', '3/3'):
            result, _ = self.run_cases('--include-ignored', shard=shard)
            actual.extend(result['cases'])
        self.assertCountEqual(actual, CASES)

    def test_costs_balance_shards_longest_first(self):
        costs = self.root / 'costs.tsv'
        # Unmeasured pool::other and pool::full_gate_prefix_host cost the
        # measured mean, 4: 9 + 1 against 4 + 4 + 2.
        costs.write_text('pool::full_gate_heap\t9\npool::full_gate_prefix\t1\npool::smoke\t2\n')
        shards = {}
        for shard in ('1/2', '2/2'):
            self.env['HIRSEL_TEST_COSTS'] = str(costs)
            result, _ = self.run_cases('--include-ignored', '--skip=host::', shard=shard)
            shards[shard] = set(result['cases'])
        self.assertEqual(shards, {'1/2': {'pool::full_gate_heap', 'pool::full_gate_prefix'},
                                  '2/2': {'pool::full_gate_prefix_host', 'pool::other', 'pool::smoke'}})

    def test_cost_tables_enable_timing_once_including_unsharded_binaries(self):
        costs = self.root / 'costs.tsv'
        costs.write_text('pool::smoke\t1\n')
        self.env['HIRSEL_TEST_COSTS'] = str(costs)
        for shard in ('', '1/2'):
            result, _ = self.run_cases(shard=shard)
            self.assertIn('--report-time', result['argv'])
            self.assertIn('-Zunstable-options', result['argv'])
            self.assertEqual(result['bootstrap'], '1')
        result, _ = self.run_cases('-Z', 'unstable-options', '--report-time')
        self.assertEqual(result['argv'].count('--report-time'), 1)

    def test_flags_and_values_do_not_select_cases(self):
        actual = []
        for shard in ('1/3', '2/3', '3/3'):
            result, _ = self.run_cases('-Z', 'unstable-options', '--test-threads', '2',
                                       '--include-ignored', shard=shard)
            actual.extend(result['cases'])
        self.assertCountEqual(actual, CASES)

    def test_filtered_shards_use_first_shard_only(self):
        for shard in ('1/3', '2/3', '3/3'):
            result, _ = self.run_cases('full_gate_', '--ignored', shard=shard)
            self.assertEqual(set(result['cases']),
                             {name for name in CASES if 'full_gate_' in name} if shard == '1/3' else set())

    def test_values_and_spaces_survive_selection(self):
        args = ('--test-threads', '2', '--logfile', 'a path with spaces', '--color=never',
                '--format', 'pretty', '--shuffle-seed', '123', '-Z', 'unstable-options',
                'full_gate_', '--ignored')
        result, _ = self.run_cases(*args)
        for value in ['a path with spaces', '2', '123', 'unstable-options']:
            self.assertIn(value, result['argv'])
        self.assertEqual(set(result['cases']), {name for name in CASES if 'full_gate_' in name})

    def test_list_and_full_gate_inventory_agree(self):
        _, output = self.run_cases('full_gate_', '--ignored', '--list')
        listed = [line[:-6] for line in output.splitlines() if line.endswith(': test')]
        self.assertEqual(set(listed), {name for name in CASES if 'full_gate_' in name})
        expected = [line.split(' ', 2)[2] for line in output.splitlines()
                    if line.startswith('HIRSEL_FULL_GATE_EXPECT fixture-test-it ')]
        self.assertEqual(set(expected), {name for name in CASES if 'full_gate_' in name})

    def test_end_of_options_and_equals_skip_preserve_selection(self):
        result, _ = self.run_cases('--ignored', '--skip=prefix', '--', 'full_gate_')
        self.assertEqual(set(result['cases']), {'host::full_gate_arm', 'pool::full_gate_heap'})

    def test_unpartitioned_command_passes_original_argv(self):
        result, _ = self.run_cases('pool::other', '--exact')
        self.assertEqual(result['cases'], ['pool::other'])
        self.assertEqual(result['argv'], ['pool::other', '--exact'])


class LauncherBless(LauncherFixture):
    """A bless on the pool would rewrite a discarded copy of the checkout."""

    def launch(self, *args, **env):
        return subprocess.run(['sh', str(LAUNCHER), 'harness', 'pool::other', *args], cwd=self.root,
                              env=self.env | env, capture_output=True, text=True)

    def test_every_bless_form_fails_on_the_pool(self):
        for args, env in [((), dict(HIRSEL_BLESS='1')), ((), dict(INSTA_UPDATE='always')),
                          (('--bless',), {}), (('--update-passing',), {})]:
            run = self.launch(*args, HIRSEL_TEST_POOL='1', **env)
            self.assertEqual(run.returncode, 2, (args, env, run.stderr))
            self.assertIn('discarded copy of the checkout', run.stderr)
            self.assertFalse(self.observed.exists())

    def test_a_bless_on_this_host_runs(self):
        run = self.launch(HIRSEL_BLESS='1', INSTA_UPDATE='always')
        self.assertEqual(run.returncode, 0, run.stderr)
        self.assertEqual(json.loads(self.observed.read_text())['cases'], ['pool::other'])


class LauncherMemory(LauncherFixture):
    def limits(self, limit):
        probe = self.root / 'probe'
        probe.write_text('#!/bin/sh\necho "$(ulimit -S -d) $(ulimit -H -d) $HIRSEL_MEMORY_LIMIT"\n')
        probe.chmod(0o755)
        env = self.env | ({'HIRSEL_MEMORY_LIMIT': limit} if limit else {})
        if not limit:
            env.pop('HIRSEL_MEMORY_LIMIT', None)
        run = subprocess.run(['sh', str(LAUNCHER), 'probe'], cwd=self.root, env=env,
                             capture_output=True, text=True, timeout=60)
        self.assertEqual(run.returncode, 0, run.stderr)
        return run.stdout.split()

    def test_the_grant_caps_the_run_data_in_every_unit(self):
        hard = subprocess.run(['sh', '-c', 'ulimit -H -d'], capture_output=True, text=True).stdout.strip()
        # A soft limit: a sanitizer's child may lift it back to the hard one.
        self.assertEqual(self.limits('1048576K'), ['1048576', hard, '1048576K'])
        self.assertEqual(self.limits('512M'), ['524288', hard, '512M'])
        self.assertEqual(self.limits('2G'), ['2097152', hard, '2G'])

    def test_without_a_grant_the_launcher_sets_no_cap(self):
        soft, hard = subprocess.run(['sh', '-c', 'echo $(ulimit -S -d) $(ulimit -H -d)'],
                                    capture_output=True, text=True).stdout.split()
        self.assertEqual(self.limits(''), [soft, hard])


class LauncherBinaries(unittest.TestCase):
    """CARGO_BIN_EXE_<name> is compiled in under HIRSEL_TEST_BIN_EXE_DIR and
    linked by the launcher to the binary the run carries (tools/buck2/defs.bzl)."""

    def test_package_binaries_resolve_from_the_package_directory(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            (root / 'llvm/bin').mkdir(parents=True)
            (root / 'llvm/lib').mkdir()
            (root / 'crates/pkg').mkdir(parents=True)
            binary = root / 'buck-out/art/__pkg-bin-tool__/tool'
            binary.parent.mkdir(parents=True)
            binary.write_text('#!/bin/sh\necho "tool ran $1"\n')
            binary.chmod(0o755)
            links = root / 'buck-out/tmp/cargo-bin-exe/pkg-test-it-harness'
            links.mkdir(parents=True)
            (links / 'tool').symlink_to(root / 'stale')
            harness = root / 'harness'
            harness.write_text('#!/bin/sh\nenv | grep "^HIRSEL_TEST_BIN" || true\n'
                               'exec ../../buck-out/tmp/cargo-bin-exe/pkg-test-it-harness/tool "$(pwd)"\n')
            harness.chmod(0o755)
            env = os.environ | dict(
                HIRSEL_TEST_TMPDIR='buck-out/tmp/cargo-target-tmpdir', HIRSEL_TEST_CWD='crates/pkg',
                LLVM_SYS_211_PREFIX=str(root / 'llvm'),
                HIRSEL_TEST_BIN_EXE_DIR='buck-out/tmp/cargo-bin-exe/pkg-test-it-harness',
                HIRSEL_TEST_BIN_EXES='tool=buck-out/art/__pkg-bin-tool__/tool')
            for key in ('HIRSEL_TEST_POOL', 'TEST_SHARD_INDEX', 'TEST_TOTAL_SHARDS', 'HIRSEL_TEST_HOST_CASES', 'HIRSEL_TEST_BINARY'):
                env.pop(key, None)
            run = subprocess.run(['sh', str(LAUNCHER), 'harness'], cwd=root, env=env,
                                 capture_output=True, text=True)
            self.assertEqual(run.returncode, 0, run.stderr)
            self.assertEqual(run.stdout, f'tool ran {root / "crates/pkg"}\n')
            self.assertEqual(os.readlink(links / 'tool'), str(binary))
            self.assertEqual(sorted(p.name for p in links.iterdir()), ['tool'])


if __name__ == '__main__':
    unittest.main()
