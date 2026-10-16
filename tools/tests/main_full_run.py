#!/usr/bin/env python3
"""Exercise the scheduled runner against a local origin and a fake resource scope."""
import json
import os
from pathlib import Path
import subprocess
import tempfile
import sys

sys.dont_write_bytecode = True
import unittest

RUNNER = Path(__file__).resolve().parents[1] / 'main-full-run.sh'
sys.path.insert(0, str(RUNNER.parent / 'buck2'))
from sync import AGREE_LIMIT, NATIVE_FULL_SHARDS  # noqa: E402


class MainFullRun(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.repo = self.root / 'repo'
        self.runs = self.root / 'runs'
        self.trees = self.root / 'trees'
        self.bin = self.root / 'bin'
        for directory in (self.repo / 'tools', self.bin):
            directory.mkdir(parents=True)
        (self.repo / 'tools/main-full-run.sh').write_text(RUNNER.read_text())
        (self.repo / 'tools/main_full_result.py').write_text(
            (RUNNER.parent / 'main_full_result.py').read_text())
        self.env = os.environ | {
            'PATH': f'{self.bin}:{os.environ["PATH"]}',
            'HIRSEL_MAIN_RUNS': str(self.runs),
            'HIRSEL_MAIN_WORKTREES': str(self.trees),
        }
        self.scope_args = self.root / 'scope-args'
        self.env['SCOPE_ARGS'] = str(self.scope_args)
        scope = self.bin / 'systemd-run'
        scope.write_text('''#!/usr/bin/env bash
printf '%s\\n' "$@" > "$SCOPE_ARGS"
while [[ ${1:-} != nice ]]; do shift; done
exec "$@"
''')
        scope.chmod(0o755)
        self.git('init', '-b', 'main')
        self.git('config', 'user.email', 'test@example.invalid')
        self.git('config', 'user.name', 'Gate test')
        self.gate('echo "gate: passed (0 s)"')
        origin = self.root / 'origin.git'
        subprocess.run(['git', 'clone', '--bare', str(self.repo), str(origin)],
                       check=True, capture_output=True)
        self.git('remote', 'add', 'origin', str(origin))

    def git(self, *args):
        return subprocess.run(['git', '-C', str(self.repo), *args], check=True,
                              capture_output=True, text=True).stdout.strip()

    def gate(self, body):
        path = self.repo / 'tools/gate'
        path.write_text('#!/usr/bin/env bash\nset -eu\n' + body + '\n')
        path.chmod(0o755)
        self.git('add', '.')
        self.git('commit', '-m', 'Gate fixture')

    def run_gate(self):
        return subprocess.run(['bash', str(self.repo / 'tools/main-full-run.sh')],
                              env=self.env, capture_output=True, text=True)

    def result(self):
        text = (self.runs / 'latest.json').read_text()
        self.assertEqual(len(text.splitlines()), 1)
        self.assertFalse((self.runs / 'main-full.pid').exists())
        self.assertEqual(list(self.trees.iterdir()), [])
        return json.loads(text)

    def test_pins_origin_and_cleans_worktree_under_required_scope(self):
        sha = self.git('rev-parse', 'HEAD')
        # Dirty local code must never run on scheduled main.
        (self.repo / 'tools/gate').write_text('#!/bin/sh\nexit 99\n')
        run = self.run_gate()
        self.assertEqual(run.returncode, 0, run.stderr)
        result = self.result()
        self.assertEqual(result['sha'], sha)
        self.assertEqual(result['result'], 'pass')
        self.assertIsNone(result['first_failing_stage'])
        self.assertEqual(result['failing_tests'], [])
        self.assertTrue(Path(result['log']).exists())
        args = self.scope_args.read_text().splitlines()
        self.assertEqual(args[:6], ['--user', '--scope', '-p', 'MemoryMax=8G',
                                   '-p', 'CPUQuota=400%'])
        self.assertEqual(args[6:], ['nice', '-n', '10', 'tools/gate', '--full'])

    def test_reports_inner_native_failure_and_failed_tests(self):
        self.gate('''echo 'test native::seed_law ... FAILED'
echo 'check-native.sh: native laws failed (exit 101, 1 s)'
echo 'gate: native and Lean checks failed (exit 101, 1 s)'
exit 101''')
        self.git('push', 'origin', 'main')
        run = self.run_gate()
        self.assertEqual(run.returncode, 101)
        result = self.result()
        self.assertEqual(result['result'], 'fail')
        self.assertEqual(result['first_failing_stage'], 'native laws')
        self.assertEqual(result['failing_tests'], ['native::seed_law'])

    def test_records_every_stage_of_a_run_that_continues_past_failures(self):
        self.gate('''echo 'gate: workspace build passed (3 s)'
echo 'gate: workspace tests failed (exit 32, 40 s)' >&2
echo 'check-native.sh: Lean RC model audit failed (exit 1, 2 s)' >&2
echo 'check-native.sh: runtime C harness passed (5 s)'
echo 'gate: native and Lean checks failed (exit 1, 9 s)' >&2
echo 'gate: native matrix: both targets and named exclusions passed (60 s)'
echo 'gate: 2 failing stages: workspace tests;native and Lean checks' >&2
exit 32''')
        self.git('push', 'origin', 'main')
        self.assertEqual(self.run_gate().returncode, 32)
        result = self.result()
        self.assertEqual(result['first_failing_stage'], 'workspace tests')
        self.assertEqual([(s['gate'], s['stage'], s['result'], s['exit'], s['seconds'])
                          for s in result['stages']], [
            ('gate', 'workspace build', 'pass', 0, 3),
            ('gate', 'workspace tests', 'fail', 32, 40),
            ('check-native.sh', 'Lean RC model audit', 'fail', 1, 2),
            ('check-native.sh', 'runtime C harness', 'pass', 0, 5),
            ('gate', 'native and Lean checks', 'fail', 1, 9),
            ('gate', 'native matrix: both targets and named exclusions', 'pass', 0, 60)])

    def test_collects_named_cli_failures(self):
        self.gate("""echo 'conformance/run/corpus.hir: wrong observation'
echo 'memcheck: conformance/run/memory.hir under x86: invalid read'
echo 'topology: native-tests/topology/units.hir: wrong observation'
echo 'generated: success-1 under arm: unavailable'
echo 'hash sweep: conformance/run/hash.hir under x86: wrong observation'
echo 'native: conformance/run/matrix.hir llvm under x86: new exclusion'
echo 'gate: native matrix failed (exit 1, 1 s)'
exit 1""")
        self.git('push', 'origin', 'main')
        self.assertEqual(self.run_gate().returncode, 1)
        self.assertEqual(self.result()['failing_tests'], [
            'conformance/run/corpus.hir', 'conformance/run/hash.hir', 'conformance/run/matrix.hir',
            'conformance/run/memory.hir', 'native-tests/topology/units.hir', 'success-1'])

    def test_collects_only_failed_agreement_clusters(self):
        report = json.dumps({'clusters': [
            {'kind': 'native/known', 'path': 'known.hir'},
            {'kind': 'native/new', 'path': 'new.hir'}]})
        for stage, native, expected in [
                ('source and IR checker agreement', False, ['known.hir', 'new.hir']),
                ('agreement campaign on heap and native code', True, ['new.hir']),
                ('agreement under second hash seed', True, ['new.hir'])]:
            with self.subTest(stage=stage):
                data = json.loads(report)
                if native:
                    data['known_kinds'] = 1
                self.gate(f"""echo '{json.dumps(data)}'
echo 'agree: new discrepancy kind: native/new'
echo 'gate: {stage} failed (exit 1, 1 s)'
exit 1""")
                self.git('push', 'origin', 'main')
                self.assertEqual(self.run_gate().returncode, 1)
                self.assertEqual(self.result()['failing_tests'], expected)

    def test_collects_failures_from_buck2_timestamped_output(self):
        # The shape of 20261015T163232Z: Buck2 prefixes every line of a test's
        # output with a timestamp and colours its summary.
        ts = '[2026-10-15T19:22:00.843+02:00] '
        self.gate(f"""printf '%s\\n' \\
  '{ts}✗ Fail: root//:hirsel-pipeline-tests-test-it-shard-1 (34:40.0s)' \\
  '{ts}---- STDOUT ----' '{ts}' '{ts}running 3 tests' \\
  '{ts}test rt_measurement_native::full_gate_binarytrees_and_inline_counts_match_on_x86_64 ... ok' \\
  '{ts}test rt_io_native::full_gate_io_leaves_agree_and_replay_on_both_targets ... FAILED' \\
  '{ts}test rt_text_native::full_gate_text_calls_failures_c_abi_and_compiled_drops_agree_on_both_targets ... FAILED' \\
  '{ts}' '{ts}failures:' '{ts}' \\
  '{ts}---- rt_io_native::full_gate_io_leaves_agree_and_replay_on_both_targets stdout ----' \\
  '{ts}thread panicked at crates/hirsel-pipeline-tests/tests/it/rt_io_native.rs:217:9:' \\
  '{ts}test result: FAILED. 1 passed; 2 failed; 0 ignored; 0 measured; 562 filtered out' \\
  '{ts}memcheck: native-tests/regressions/leak.hir under x86_64/regions/elide/reuse/flat-walk/llvm-O0/whole/diagnostic: exit 97' \\
  '{ts}✓ Pass: root//:hirsel-lir-unittest (0.1s)' \\
  'Tests finished: \\033[38;5;10mPass 64\\033[39m. \\033[38;5;9mFail 1\\033[39m. Timeout 0.' \\
  '1 TESTS FAILED' \\
  '  ✗ root//:hirsel-pipeline-tests-test-it-shard-1 (root//tools/buck2:dev#b48a97a5614255e0)' \\
  '{ts}✗ Timeout: root//:gate-native-full (14400.0s)' \\
  'gate: full gate crate tests failed (exit 32, 2081 s)'
exit 32""")
        self.git('push', 'origin', 'main')
        self.assertEqual(self.run_gate().returncode, 32)
        result = self.result()
        self.assertEqual(result['first_failing_stage'], 'full gate crate tests')
        self.assertEqual(result['failing_tests'], [
            'native-tests/regressions/leak.hir',
            'root//:gate-native-full',
            'root//:hirsel-pipeline-tests-test-it-shard-1',
            'rt_io_native::full_gate_io_leaves_agree_and_replay_on_both_targets',
            'rt_text_native::full_gate_text_calls_failures_c_abi_and_compiled_drops_agree_on_both_targets'])
        self.assertEqual(result['infra_failures'], [])

    def test_separates_targets_the_pool_never_started(self):
        ts = '[2026-10-16T13:19:09.431+02:00] '
        self.gate(f"""printf '%s\\n' \\
  '{ts}✗ Fail: root//:gate-agree-full-shard-43 (0.0s)' '{ts}---- STDOUT ----' '{ts}' '{ts}---- STDERR ----' \\
  '{ts}Remote Execution Error on Execute with digest cb43:147 for ReSession GRPC-SESSION-ID' \\
  '{ts}✗ Fail: root//:gate-agree-full-shard-7 (812.0s)' '{ts}---- STDOUT ----' '{ts}' '{ts}---- STDERR ----' \\
  '{ts}agree: 1 discrepancy cluster' \\
  '  ✗ root//:gate-agree-full-shard-43 (root//tools/buck2:dev#b48a97a5614255e0)' \\
  '  ✗ root//:gate-agree-full-shard-7 (root//tools/buck2:dev#b48a97a5614255e0)' \\
  'gate: source and IR checker agreement (unrestricted) failed (exit 32, 250 s)'
exit 32""")
        self.git('push', 'origin', 'main')
        self.assertEqual(self.run_gate().returncode, 32)
        result = self.result()
        self.assertEqual(result['failing_tests'], ['root//:gate-agree-full-shard-7'])
        self.assertEqual(result['infra_failures'], ['root//:gate-agree-full-shard-43'])

    def test_active_lock_refuses_to_fetch_or_replace_latest(self):
        self.runs.mkdir()
        latest = self.runs / 'latest.json'
        latest.write_text('previous\n')
        with (self.runs / 'main-full.lock').open('w') as lock:
            import fcntl
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            run = self.run_gate()
        self.assertEqual(run.returncode, 75)
        self.assertEqual(latest.read_text(), 'previous\n')
        self.assertFalse(self.trees.exists())

    def test_stale_pid_does_not_prevent_run(self):
        self.runs.mkdir()
        (self.runs / 'main-full.pid').write_text('999999999\n')
        self.assertEqual(self.run_gate().returncode, 0)
        self.assertEqual(self.result()['result'], 'pass')

    def test_scope_failure_still_records_and_cleans(self):
        (self.bin / 'systemd-run').write_text('#!/bin/sh\nexit 7\n')
        self.assertEqual(self.run_gate().returncode, 7)
        result = self.result()
        self.assertEqual(result['first_failing_stage'], 'resource scope')
        self.assertEqual(result['result'], 'fail')


class GateCommands(unittest.TestCase):
    """`tools/gate --full` with a fake Buck2 and fake helpers."""

    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        for directory in ('.buck2/bin', 'tools/tests', 'tools/buck2'):
            (self.root / directory).mkdir(parents=True)
        for test in ('main_full_run', 'buck2_sync', 'gate'):
            (self.root / f'tools/tests/{test}.py').write_text('# fixture gate tests\n')
        for helper in ('gating', 'impact'):
            (self.root / f'tools/buck2/{helper}.py').write_text((RUNNER.parent / f'buck2/{helper}.py').read_text())
        for helper in ('prepare', 'sync', 'clippy', 'fmt', 'conformance', 'agree'):
            (self.root / f'tools/buck2/{helper}.py').write_text(f'''import os, sys
AGREE_LIMIT = {AGREE_LIMIT}
AGREE_SHARDS = CONFORMANCE_SHARDS = 16
NATIVE_FULL_SHARDS = {NATIVE_FULL_SHARDS}
summarize = None
if __name__ == "__main__":
    with open(os.environ["COMMANDS"], "a") as log:
        print(" ".join(["{helper}.py", *sys.argv[1:]]), file=log)
    if "{helper}" == "clippy" and os.environ.get("FAIL_CLIPPY") == "1":
        sys.exit(19)
''')
        (self.root / 'tools/buck2/full_gate_counts.py').write_text('''import os, subprocess, sys
with open(os.environ["COMMANDS"], "a") as log:
    print("full_gate_counts.py " + " ".join(sys.argv[1:]), file=log)
if os.environ.get("FAIL_COUNTS") == "1":
    sys.exit(23)
sys.exit(subprocess.call(sys.argv[sys.argv.index("--run") + 1:]))
''')
        gate = self.root / 'tools/gate'
        gate.write_text((RUNNER.parent / 'gate').read_text())
        gate.chmod(0o755)
        self.commands = self.root / 'commands'
        self.env = os.environ | {'COMMANDS': str(self.commands), 'HIRSEL_TEST_SEED': '7'}
        buck2 = self.root / '.buck2/bin/buck2'
        buck2.write_text('''#!/usr/bin/env bash
if [[ $1 == uquery ]]; then
  printf 'root//:a-unittest\\nroot//:b-test-it\\n'
  if [[ $* == *doctest* ]]; then printf 'root//:doclib\\n'; fi
  exit 0
fi
printf '%s\\n' "$*" >> "$COMMANDS"
''')
        buck2.chmod(0o755)
        native = self.root / 'tools/check-native.sh'
        native.write_text('#!/bin/sh\nprintf "native gate\\n" >> "$COMMANDS"\n')
        native.chmod(0o755)

    def run_full(self):
        return subprocess.run([sys.executable, 'tools/gate', '--full'], cwd=self.root,
                              env=self.env, capture_output=True, text=True)

    def test_full_keeps_unrestricted_campaigns_and_all_old_gates(self):
        result = self.run_full()
        self.assertEqual(result.returncode, 0, result.stderr)
        commands = self.commands.read_text().splitlines()
        full = 'test -c hirsel.full=true -c hirsel.test_seed=7 '
        for command in ['prepare.py', 'sync.py --check', 'clippy.py', 'fmt.py --check', 'conformance.py', 'agree.py --full',
                        'build -c hirsel.full=true -c hirsel.test_seed=7 //: --materializations none', 'native gate',
                        full + 'root//:a-unittest root//:b-test-it root//:doclib -- --timeout 1800',
                        full + '--target-platforms //tools/buck2:release root//:a-unittest root//:b-test-it root//:doclib -- --timeout 1800',
                        full + '//:gate-agree-native-full -- --timeout 14400',
                        full + ' '.join(f'//:gate-native-full-shard-{i}' for i in range(1, NATIVE_FULL_SHARDS + 1)) + ' -- --timeout 14400',
                        full + '//:gate-native-smoke -- --timeout 1800',
                        full + '//:gate-conformance-hash-seed -- --timeout 14400',
                        full + '//:gate-native-hash-sweep -- --timeout 14400']:
            self.assertIn(command, commands)
        self.assertIn('gate: source and IR checker agreement (unrestricted)\n', result.stdout)
        self.assertNotIn('agree.py', commands)
        self.assertFalse(any('doclib' in c and 'full_gate_' in c for c in commands))
        # Every Buck2 command of the full gate sees the campaign targets.
        self.assertTrue(all(' -c hirsel.full=true -c hirsel.test_seed=7 ' in command for command in commands
                            if command.split()[0] in ('build', 'test')))
        self.assertIn('gate: randomly seeded tests run with PROPTEST_RNG_SEED=7\n', result.stdout)
        self.assertIn('gate: tool tests: tools/tests/gate.py passed', result.stdout)

    def test_full_gate_runs_every_stage_past_failures_and_fails_at_the_end(self):
        self.env['FAIL_COUNTS'] = '1'
        self.env['FAIL_CLIPPY'] = '1'
        result = self.run_full()
        self.assertEqual(result.returncode, 19)
        self.assertIn('gate: workspace clippy failed (exit 19,', result.stderr)
        self.assertIn('gate: full gate crate tests failed (exit 23,', result.stderr)
        self.assertIn('gate: 3 failing stages: workspace clippy;full gate crate tests;'
                      'full gate crate tests in release\n', result.stderr)
        self.assertNotIn('gate: passed', result.stdout)
        commands = self.commands.read_text().splitlines()
        # Every later stage still ran, up to the last.
        for command in ['fmt.py --check', 'conformance.py', 'agree.py --full', 'native gate',
                        'test -c hirsel.full=true -c hirsel.test_seed=7 //:gate-native-hash-sweep -- --timeout 14400']:
            self.assertIn(command, commands)

    def test_named_ignored_group_runs_in_both_profiles(self):
        group = ' -- --timeout 14400 --test-arg=full_gate_ --test-arg=--ignored'
        result = self.run_full()
        self.assertEqual(result.returncode, 0, result.stderr)
        commands = self.commands.read_text().splitlines()
        self.assertEqual(len([c for c in commands if c.startswith('full_gate_counts.py --run ')]), 2)
        self.assertIn('test -c hirsel.full=true -c hirsel.test_seed=7 root//:a-unittest root//:b-test-it' + group, commands)
        self.assertIn('test -c hirsel.full=true -c hirsel.test_seed=7 --target-platforms //tools/buck2:release '
                      'root//:a-unittest root//:b-test-it' + group, commands)
        # An unrestricted --ignored would also run fixture regeneration
        # and manual diagnostics. Only the named group is automatic.
        self.assertFalse(any(command.endswith('--test-arg=--ignored')
                             and 'full_gate_' not in command for command in commands))


class NativeGate(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.commands = self.root / 'commands'
        self.env = os.environ | {'COMMANDS': str(self.commands)}
        (self.root / 'tools').mkdir()
        (self.root / 'tools/check-native.sh').write_text((RUNNER.parent / 'check-native.sh').read_text())
        for tool in ('.buck2/bin/buck2', 'crates/hirsel-rt/harness/run.sh',
                     'tools/native-toolchain/check', 'proofs/rc/audit.sh', 'proofs/rc/check-vectors.sh'):
            path = self.root / tool
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(f'''#!/usr/bin/env bash
if [[ $1 == uquery ]]; then echo 'root//:x-test-it'; exit 0; fi
printf '%s\\n' "{Path(tool).name} $*" >> "$COMMANDS"
if [[ "{tool} $*" == *${{FAIL:-no-failing-command}}* ]]; then exit 7; fi
if [[ $1 == build ]]; then echo /archives; fi
''')
            path.chmod(0o755)

    def run_native(self, fail):
        self.env['FAIL'] = fail
        return subprocess.run(['bash', 'tools/check-native.sh'], cwd=self.root,
                               env=self.env, capture_output=True, text=True)

    def test_every_stage_runs_and_a_failing_command_still_ends_its_stage(self):
        result = self.run_native('build -c hirsel.full=true //:hirsel-rt-archives')
        self.assertEqual(result.returncode, 7)
        commands = self.commands.read_text().splitlines()
        # The archive build failed, so its stage never ran the harness on no archives.
        self.assertFalse(any(c.startswith('run.sh') for c in commands))
        self.assertIn('check-native.sh: runtime C harness failed (exit 7,', result.stderr)
        self.assertIn('check-native.sh: 1 failing stages: runtime C harness\n', result.stderr)
        # Every later stage still ran.
        for tool in ('check --prefix', 'audit.sh', 'check-vectors.sh'):
            self.assertTrue(any(c.startswith(tool) for c in commands), tool)
        self.assertIn('check-native.sh: Lean RC model vectors passed', result.stdout)
        self.assertTrue(commands[-1].startswith('buck2 test') and 'rc_maps_native::' in commands[-1])

    def test_a_clean_run_passes(self):
        result = self.run_native('no-failing-command')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn('check-native.sh: passed', result.stdout)
        self.assertTrue(any(c.startswith('run.sh') for c in self.commands.read_text().splitlines()))


class AgreementShards(unittest.TestCase):
    def setUp(self):
        sys.path.insert(0, str(RUNNER.parent / 'buck2'))
        import agree
        self.agree = agree
        self.report = dict(seed=1, limit=4, hash_seed=0, programs=3, generated=9,
                           checked=2, skipped=0, rejected=1, valid=1, unavailable=0,
                           runtime_limits=0, executed=1, edits={'mode': 2}, clusters=[])

    def outputs(self):
        return {f'shard-{i}': '---- STDOUT ----\n' + json.dumps(
            self.report | {'shard': dict(index=i, count=2)}, indent=2) for i in (1, 2)}

    def test_sums_classifications_and_edits_but_keeps_global_catalogue_counts(self):
        result = self.agree.summarize(self.outputs(), 2, 4)
        self.assertEqual(result, self.report | dict(checked=4, rejected=2, valid=2,
                                                   executed=2, edits={'mode': 4}))

    def test_refuses_missing_duplicate_mismatched_and_flagged_shards(self):
        for changed in [
                {'shard': dict(index=1, count=2)},
                {'shard': dict(index=2, count=3)},
                {'generated': 10}, {'checked': 1}, {'edits': {}},
                {'clusters': [dict(kind='bug', path='reproducer.hir')]}]:
            with self.subTest(changed=changed):
                outputs = self.outputs()
                outputs['shard-2'] = json.dumps(self.report | {'shard': dict(index=2, count=2)} | changed,
                                               indent=2)
                with self.assertRaises(ValueError):
                    self.agree.summarize(outputs, 2, 4)
        with self.assertRaisesRegex(ValueError, '1 of 2'):
            self.agree.summarize({'shard-1': self.outputs()['shard-1']}, 2, 4)
        with self.assertRaisesRegex(ValueError, 'expected one'):
            self.agree.report_from_output('no report')
        with self.assertRaisesRegex(ValueError, 'expected one'):
            self.agree.report_from_output(self.outputs()['shard-1'] + '\n' + self.outputs()['shard-2'])


    def test_the_unrestricted_campaign_covers_the_whole_catalogue(self):
        self.report |= dict(limit=self.agree.UNLIMITED, generated=4)
        result = self.agree.summarize(self.outputs(), 2, self.agree.UNLIMITED)
        self.assertEqual(result['checked'], 4)
        with self.assertRaisesRegex(ValueError, 'sample parameters'):
            self.agree.summarize(self.outputs(), 2, 4)
        self.report |= dict(generated=5)
        with self.assertRaisesRegex(ValueError, 'did not cover'):
            self.agree.summarize(self.outputs(), 2, self.agree.UNLIMITED)


class ActionSizing(unittest.TestCase):
    def setUp(self):
        sys.path.insert(0, str(RUNNER.parent / 'buck2'))
        import action_sizes_from_log
        self.module = action_sizes_from_log
        self.action = dict(target='hirsel-check', profile='dev', phase='rlib [pic]',
                           wall=10., end=100., cpu=8, memory_kb=1048576)
        self.record = dict(end=100, requested_cpu='8', requested_kb='1048576',
                           wall_ms='10000', cpu_usec='43000000', peak_bytes='900000000',
                           anon_peak_bytes='600000000', worker='fixture')

    def test_unique_usage_sizes_cpu_and_memory_and_remembers_evidence(self):
        joined = self.module.join([self.action], {('worker', 'op'): self.record})
        row = self.module.sizes(joined)['profiles']['dev']['hirsel-check']
        self.assertEqual((row['cpu_count'], row['memory_kb']), (4, 1310720))
        mature = self.module.sizes([(self.action, self.record)] * 20)['profiles']['dev']['hirsel-check']
        self.assertEqual(mature['cpu_count'], 4)
        self.assertEqual(row['measurements'][0]['cpu_seconds'], 43.)

    def test_sparse_parallel_probe_does_not_become_a_reservation_floor(self):
        record = self.record | dict(cpu_usec='16730000')
        row = self.module.sizes([(self.action, record)])['profiles']['dev']['hirsel-check']
        self.assertEqual((row['cpu_count'], row['memory_kb']), (2, 1310720))
        self.assertEqual(row['measurements'][0]['requested_cpu'], 8)

    def test_fast_parallel_compile_is_capped_without_changing_memory(self):
        record = self.record | dict(cpu_usec='51240000')
        row = self.module.sizes([(self.action, record)])['profiles']['dev']['hirsel-check']
        self.assertEqual((row['cpu_count'], row['memory_kb']), (4, 1310720))

    def test_twenty_samples_use_p95_rather_than_the_outlier(self):
        sample = (self.action, self.record | dict(cpu_usec='20100000'))
        outlier = (self.action, self.record | dict(cpu_usec='75000000'))
        row = self.module.sizes([sample] * 19 + [outlier])['profiles']['dev']['hirsel-check']
        self.assertEqual(row['cpu_count'], 3)
        self.assertEqual(row['p95_cores'], 2.01)

    def test_stored_evidence_keeps_a_fractional_core_above_an_integer(self):
        record = self.record | dict(cpu_usec='20001000')
        row = self.module.sizes([(self.action, record)])['profiles']['dev']['hirsel-check']
        self.assertEqual(row['cpu_count'], 3)
        self.assertEqual(self.module.cpu_request(row['p95_cores']), 3)

    def test_one_cpu_covers_use_up_to_the_slot_share(self):
        for cpu_usec, cpus in [('16000000', 1), ('16010000', 2), ('11000000', 1)]:
            row = self.module.sizes([(self.action, self.record | dict(cpu_usec=cpu_usec))])['profiles']['dev']['hirsel-check']
            self.assertEqual(row['cpu_count'], cpus, cpu_usec)

    def test_calibrated_reservation_replaces_the_rule_and_may_exceed_four(self):
        calibrated = self.module.CALIBRATED_CPUS['dev']
        calibrated['hirsel-check'] = dict(cpu_count=8)
        try:
            row = self.module.sizes([(self.action, self.record | dict(cpu_usec='10000000'))])['profiles']['dev']['hirsel-check']
        finally:
            del calibrated['hirsel-check']
        self.assertEqual(row['cpu_count'], 8)
        self.assertEqual(self.module.compile_cpus('dev', 'hirsel-check', row['p95_cores']), 1)

    def test_merge_replaces_only_calibrated_rows(self):
        existing = dict(profiles={'dev': {'a': 1, 'b': 2}, 'release': {'a': 3}}, event_logs=['old'], unmatched=[])
        measured = dict(profiles={'dev': {'a': 4, 'b': 5}, 'release': {'a': 6}}, event_logs=['probe'],
                        unmatched=[dict(profile='dev', target='b'), dict(profile='dev', target='a')])
        from unittest.mock import patch
        with patch.dict(self.module.CALIBRATED_CPUS, {'dev': {'b': {}}, 'release': {}}):
            merged = self.module.merge(existing, measured)
        self.assertEqual(merged['profiles'], {'dev': {'a': 1, 'b': 5}, 'release': {'a': 3}})
        self.assertEqual((merged['event_logs'], merged['unmatched']),
                         (['old', 'probe'], [dict(profile='dev', target='b')]))

    def test_a_calibrated_request_is_its_fastest_measured_count(self):
        row = dict(cpu_count=4, wall_with_queue_seconds={2: 55.7, 4: 40.6, 8: 64.8})
        self.module.check_calibrated({'release': {'hirsel-rc': row}})
        with self.assertRaises(ValueError):
            self.module.check_calibrated({'release': {'hirsel-rc': row | dict(cpu_count=2)}})
        eight = dict(cpu_count=4, wall_with_queue_seconds={2: 7.5, 4: 5.3, 8: 3.2})
        self.module.check_calibrated({'dev': {'hirsel-engine': eight}})
        self.module.check_calibrated({'dev': {'hirsel-engine': eight | dict(cpu_count=8)}})
        with self.assertRaises(ValueError):
            self.module.check_calibrated({'dev': {'hirsel-engine': eight | dict(cpu_count=2)}})

    def test_ambiguous_missing_and_reused_usage_are_not_measurements(self):
        for records in [{}, {'op1': self.record, 'op2': self.record}]:
            result = self.module.sizes(self.module.join([self.action], records))
            self.assertEqual(result['profiles']['dev'], {})
            self.assertEqual(len(result['unmatched']), 1)
        result = list(self.module.join([self.action, self.action], {'op': self.record}))
        self.assertIsNone(result[1][1])

    def test_escalation_and_legacy_cgroup_peaks_are_conservative(self):
        record = self.record | dict(requested_kb='2097152', anon_peak_bytes='-')
        rows = self.module.sizes(self.module.join([self.action], {'op': record}))
        row = rows['profiles']['dev']['hirsel-check']
        self.assertEqual(row['p99_peak_bytes'], 900000000)
        self.assertGreaterEqual(row['memory_kb'] * 1024, 900000000 * 1.25)


if __name__ == '__main__':
    unittest.main()
