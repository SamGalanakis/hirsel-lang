#!/usr/bin/env python3
"""Caller selection, explicit inputs and shard coverage at the launcher seam."""
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from types import SimpleNamespace

ROOT = Path(__file__).resolve().parents[2]
LAUNCHER = Path(os.environ.get('HIRSEL_TEST_LAUNCHER', ROOT / 'tools/buck2/test_launcher.sh')).resolve()
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
        json.dump(dict(argv=sys.argv[1:], bootstrap=os.environ.get('RUSTC_BOOTSTRAP'), threads=os.environ.get('RUST_TEST_THREADS'), jobs=os.environ.get('HIRSEL_JOBS'), cases=[name for name in selected
                  if args.ignored or args.include_ignored or not cases[name]]), output)
    for name in selected:
        status = 'ok' if args.ignored or args.include_ignored or not cases[name] else 'ignored'
        print('test ' + name + ' ... ' + status)
'''


class CpuRequests(unittest.TestCase):
    def platforms(self, mode):
        source_dir = ROOT / 'tools/buck2'
        paths = {name: Path(os.environ.get('HIRSEL_TEST_' + name.upper(), source_dir / (name + '.bzl')))
                 for name in ('platforms', 'test_requirements', 'test_sizes', 'exec_sizes')}
        config = {'tests': mode, 'remote_properties': 'kiln_executor_runtime=fixture-runtime'}
        namespace = dict(read_root_config=lambda section, key, default: config.get(key, default),
                         CommandExecutorConfig=lambda **kwargs: kwargs,
                         ConfigurationInfo=object(), rule=lambda **kwargs: None,
                         attrs=SimpleNamespace(**{name: lambda *args, **kwargs: None
                                                  for name in ('dict', 'string', 'dep', 'list')}))
        def load(label, *names):
            imported = {}
            exec(paths[label.removeprefix(':').removesuffix('.bzl')].read_text(), imported)
            namespace.update({name: imported[name] for name in names})
        namespace['load'] = load
        # This module uses the Python-compatible Starlark subset. Evaluate the
        # production request functions; Buck2 cquery also validates Starlark.
        exec(paths['platforms'].read_text(), namespace)
        return namespace

    def test_exact_requirements_override_measurement_and_focus(self):
        for mode in ('pool', 'focus'):
            module = self.platforms(mode)
            for binary, cpu in [('hirsel-cli-test-free', 8), ('hirsel-cli-test-cli', 4)]:
                measured = module['TEST_REQUESTS'][binary]
                request = module['test_request'](binary)
                self.assertEqual(request, [cpu, *measured[1:]])
                props = module['test_executor'](*request, binary=binary)['remote_execution_properties']
                self.assertEqual((props['cpu_count'], props['exact_cpus'], props['cpu_affinity']),
                                 (str(cpu), str(cpu), 'exact-v1'))
                self.assertEqual(props['kiln_executor_runtime'], 'fixture-runtime')

    def test_ordinary_targets_keep_measured_and_focus_defaults(self):
        for mode in ('pool', 'focus'):
            module = self.platforms(mode)
            for binary in ('hirsel-rt-test-it', 'unknown-test'):
                measured = module['TEST_REQUESTS'].get(binary, module['DEFAULT_TEST_REQUEST'])
                request = module['test_request'](binary)
                self.assertEqual(request, [1 if mode == 'focus' else measured[0], *measured[1:]])
                props = module['test_executor'](*request, binary=binary)['remote_execution_properties']
                self.assertNotIn('cpu_affinity', props)
                self.assertNotIn('exact_cpus', props)


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
            LLVM_SYS_231_PREFIX=str(self.root / 'llvm'), CASES=json.dumps(CASES),
            OBSERVED=str(self.observed), HIRSEL_TEST_BINARY='fixture-test-it')
        for key in ('HIRSEL_TEST_POOL', 'HIRSEL_TEST_CPUS', 'HIRSEL_TEST_EXACT_CPUS', 'TEST_SHARD_INDEX', 'TEST_TOTAL_SHARDS', 'HIRSEL_TEST_HOST_ONLY'):
            self.env.pop(key, None)


class LauncherSelection(LauncherFixture):
    def launcher_with_quota(self, quota):
        # Keep the real launcher's quota logic, with deterministic kernel files.
        # A wide affinity mask does not imply a wide ancestor cpu.max quota.
        (self.root / 'cgroup').write_text('0::/child\n')
        (self.root / 'groups/child').mkdir(parents=True, exist_ok=True)
        (self.root / 'groups/child/cpu.max').write_text('max 100000\n')
        (self.root / 'groups/cpu.max').write_text(f'{quota} 100000\n')
        launcher = self.root / 'launcher.sh'
        launcher.write_text(LAUNCHER.read_text()
                            .replace('/proc/self/cgroup', str(self.root / 'cgroup'))
                            .replace('/sys/fs/cgroup', str(self.root / 'groups')))
        return launcher

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

    def test_two_cpu_reservation_bounds_threads_and_workers(self):
        self.env.update(HIRSEL_TEST_CPUS='2', RUST_TEST_THREADS='8', HIRSEL_JOBS='8')
        result, _ = self.run_cases()
        self.assertEqual(result['threads'], '2')
        self.assertEqual(result['jobs'], '2')

    @unittest.skipUnless(len(os.sched_getaffinity(0)) >= 8, 'requires eight inherited CPUs')
    def test_exact_eight_cpu_request_preserves_width_or_refuses_a_tighter_quota(self):
        self.env.update(HIRSEL_TEST_CPUS='8', HIRSEL_TEST_EXACT_CPUS='8',
                        RUST_TEST_THREADS='8', HIRSEL_JOBS='8')
        cpus = ','.join(map(str, sorted(os.sched_getaffinity(0))[:8]))
        for quota in ('max', '400000', '100000'):
            with self.subTest(quota=quota):
                self.observed.unlink(missing_ok=True)
                launcher = self.launcher_with_quota(quota)
                run = subprocess.run(['taskset', '-c', cpus, 'sh', str(launcher), 'harness'],
                                     cwd=self.root, env=self.env, capture_output=True, text=True)
                if quota != 'max':
                    self.assertEqual(run.returncode, 2, run.stderr)
                    self.assertIn(f'exceeds inherited quota ({int(quota) // 100000})', run.stderr)
                    self.assertFalse(self.observed.exists())
                else:
                    self.assertEqual(run.returncode, 0, run.stderr)
                    result = json.loads(self.observed.read_text())
                    self.assertEqual((result['threads'], result['jobs']), ('8', '8'))

    @unittest.skipUnless(len(os.sched_getaffinity(0)) >= 2, 'requires two inherited CPUs')
    def test_exact_two_cpu_request_preserves_the_inherited_mask(self):
        self.env.update(HIRSEL_TEST_CPUS='2', HIRSEL_TEST_EXACT_CPUS='2',
                        RUST_TEST_THREADS='2', HIRSEL_JOBS='2')
        cpus = ','.join(map(str, sorted(os.sched_getaffinity(0))[:2]))
        launcher = self.launcher_with_quota('max')
        run = subprocess.run(['taskset', '-c', cpus, 'sh', str(launcher), 'harness'],
                             cwd=self.root, env=self.env, capture_output=True, text=True)
        self.assertEqual(run.returncode, 0, run.stderr)
        result = json.loads(self.observed.read_text())
        self.assertEqual((result['threads'], result['jobs']), ('2', '2'))

    def test_exact_request_refuses_a_smaller_inherited_affinity(self):
        self.env.update(HIRSEL_TEST_CPUS='8', HIRSEL_TEST_EXACT_CPUS='8')
        cpu = str(min(os.sched_getaffinity(0)))
        run = subprocess.run(['taskset', '-c', cpu, 'sh', str(LAUNCHER), 'harness'],
                             cwd=self.root, env=self.env, capture_output=True, text=True)
        self.assertEqual(run.returncode, 2, run.stderr)
        self.assertIn('exact CPU request', run.stderr)
        self.assertFalse(self.observed.exists())

    def test_pool_without_a_reservation_fails_before_running_the_harness(self):
        run = subprocess.run(['sh', str(LAUNCHER), 'harness'], cwd=self.root,
                             env=self.env | {'HIRSEL_TEST_POOL': '1'}, capture_output=True, text=True)
        self.assertEqual(run.returncode, 2, run.stderr)
        self.assertIn('pool action requires HIRSEL_TEST_CPUS', run.stderr)
        self.assertFalse(self.observed.exists())

    def test_caller_can_choose_less_than_the_reservation(self):
        self.env.update(HIRSEL_TEST_CPUS='2', RUST_TEST_THREADS='1', HIRSEL_JOBS='1')
        result, _ = self.run_cases('--test-threads=1')
        self.assertEqual((result['threads'], result['jobs']), ('1', '1'))
        self.assertIn('1', result['argv'])

    def test_reservation_also_sets_defaults_without_caller_settings(self):
        self.env.update(HIRSEL_TEST_CPUS='2')
        for key in ('RUST_TEST_THREADS', 'HIRSEL_JOBS'):
            self.env.pop(key, None)
        result, _ = self.run_cases()
        self.assertEqual((result['threads'], result['jobs']), ('2', '2'))

    def test_thread_arguments_cannot_exceed_the_reservation(self):
        self.env.update(HIRSEL_TEST_CPUS='2')
        for args in [('--test-threads', '8'), ('--test-threads=8',)]:
            for shard in ['', '1/2']:
                result, _ = self.run_cases(*args, shard=shard)
                argv = result['argv']
                threads = (argv[argv.index('--test-threads') + 1] if '--test-threads' in argv
                           else next(a.split('=')[1] for a in argv if a.startswith('--test-threads=')))
                self.assertEqual(threads, '2')

    def test_flag_shaped_values_remain_values(self):
        self.env['HIRSEL_TEST_CPUS'] = '2'
        probe = self.root / 'probe'
        probe.write_text('#!/usr/bin/env python3\nimport json,sys\nprint(json.dumps(sys.argv[1:]))\n')
        probe.chmod(0o755)
        for args in [('--logfile', '--test-threads=8'), ('--skip', '--test-threads=8'),
                     ('--', '--test-threads=8')]:
            run = subprocess.run(['sh', str(LAUNCHER), 'probe', *args], cwd=self.root,
                                 env=self.env, capture_output=True, text=True)
            self.assertEqual(run.returncode, 0, run.stderr)
            self.assertEqual(json.loads(run.stdout), list(args))

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
                          (('--bless',), {})]:
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


class LauncherCache(LauncherFixture):
    def launch_probe(self, body, *, env=None, args=()):
        probe = self.root / 'probe'
        probe.write_text('#!/usr/bin/env python3\n' + body)
        probe.chmod(0o755)
        return subprocess.run(['sh', str(LAUNCHER), 'probe', *args], cwd=self.root,
                              env=self.env if env is None else env,
                              capture_output=True, text=True)

    def test_unset_empty_and_unwritable_cache_variables_are_action_owned(self):
        shared = self.root / 'shared'
        shared.mkdir()
        blocked = self.root / 'blocked'
        blocked.write_text('not a directory')
        for value in (None, '', str(blocked), str(shared)):
            env = self.env.copy()
            for key in ('HOME', 'XDG_CACHE_HOME'):
                env.pop(key, None)
                if value is not None:
                    env[key] = value
            env['HIRSEL_RESULTS_DIR'] = str(shared / 'results')
            env['HIRSEL_NATIVE_CACHE'] = str(shared / 'runtime')
            run = self.launch_probe('''
import json, os
from pathlib import Path
cache = Path(os.environ['XDG_CACHE_HOME'])
assert cache.is_absolute() and cache.is_dir()
assert cache.stat().st_mode & 0o777 == 0o700
for key in ('XDG_CACHE_HOME', 'HIRSEL_RESULTS_DIR', 'HIRSEL_NATIVE_CACHE'):
    path = Path(os.environ[key]); path.mkdir(parents=True, exist_ok=True)
    (path / 'written').write_text('owned')
print(json.dumps(dict(cache=str(cache), results=os.environ['HIRSEL_RESULTS_DIR'], runtime=os.environ['HIRSEL_NATIVE_CACHE'])))
''', env=env)
            self.assertEqual(run.returncode, 0, run.stderr)
            paths = json.loads(run.stdout)
            action = Path(paths['cache']).parent
            self.assertEqual(Path(paths['results']).parent, action)
            self.assertEqual(Path(paths['runtime']).parent, action)
            self.assertFalse(action.exists())
            self.assertEqual(list(shared.iterdir()), [])
            self.assertEqual(blocked.read_text(), 'not a directory')

    def test_failure_and_filtered_empty_shard_clean_up_and_preserve_exit(self):
        for args, env in [((), {}), (('only-this',), dict(TEST_TOTAL_SHARDS='2', TEST_SHARD_INDEX='1'))]:
            run = self.launch_probe("import os, sys\nif '--list' in sys.argv:\n    print('only-this: test'); sys.exit(0)\nprint(os.environ['XDG_CACHE_HOME'])\nsys.exit(17)\n",
                                    args=args, env=self.env | env)
            if not args:
                self.assertEqual(run.returncode, 17, run.stderr)
                self.assertFalse(Path(run.stdout.strip()).parent.exists())
            else:
                self.assertEqual(run.returncode, 0, run.stderr)
            scratch = Path(os.readlink(self.root / 'tmp'))
            self.assertEqual(list(scratch.glob('action-*')), [])

    def test_parallel_launchers_never_share_a_cache(self):
        probe = self.root / 'probe'
        probe.write_text('''#!/usr/bin/env python3
import os
from pathlib import Path
cache=Path(os.environ['XDG_CACHE_HOME'])
(cache / 'owner').write_text(str(os.getpid()))
print(cache)
''')
        probe.chmod(0o755)
        runs = [subprocess.Popen(['sh', str(LAUNCHER), 'probe'], cwd=self.root,
                                 env=self.env, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
                for _ in range(8)]
        caches = []
        for run in runs:
            stdout, stderr = run.communicate()
            self.assertEqual(run.returncode, 0, stderr)
            cache = Path(stdout.strip())
            caches.append(cache)
            self.assertFalse(cache.parent.exists())
        self.assertEqual(len(set(caches)), len(caches))


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
                LLVM_SYS_231_PREFIX=str(root / 'llvm'),
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
