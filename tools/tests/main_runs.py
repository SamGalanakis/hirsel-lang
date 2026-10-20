#!/usr/bin/env python3
"""The hourly main runner (tools/main-runs) against a local origin, and the full gate's stages."""
import fcntl
import json
import os
import runpy
from pathlib import Path
import subprocess
import tempfile
import sys
import time

sys.dont_write_bytecode = True
import unittest

TOOLS = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(TOOLS / 'buck2'))
from sync import AGREE_LIMIT, NATIVE_FULL_SHARDS  # noqa: E402

# Buck2 for the runner's scratch repository: `test` runs each named target,
# which fails when the tree holds RED-<name> and prints DETAILS-<name> when it
# holds that, and writes its result where the full gate's stage keeps its logs;
# `log show` replays a log.
FAKE_BUCK2 = r'''#!/usr/bin/env bash
if [[ $1 == log && $2 == show ]]; then cat "$3"; exit 0; fi
if [[ $1 == uquery ]]; then echo "${2}"; exit 0; fi
if [[ $1 == test ]]; then
  log="$HIRSEL_BUCK2_LOG_DIR/$(date +%s%N)-test.pb.zst"
  status=0
  printf '%s\n' "test $PWD ${*:2}" >> "$COMMANDS"
  for argument in "${@:2}"; do
    [[ $argument == root//:* ]] || continue
    name=${argument#root//:}
    if [[ -e RED-$name ]]; then result=2; details="test case_x ... FAILED"; status=1
    elif [[ -e DETAILS-$name ]]; then result=1; details=$(cat "DETAILS-$name")
    else result=1; details="test case_x ... ok"; fi
    printf '{"Event":{"data":{"Instant":{"data":{"TestResult":{"name":"%s","status":%s,"details":"%s","duration_us":1000}}}}}}\n' \
      "$argument" "$result" "$details" >> "$log"
  done
  exit $status
fi
'''
# The full gate: one Buck2 stage over targets a and b (and gate-perf-counts when
# the tree prints its measurement), recorded as tools/gate --full records it.
FAKE_GATE = r'''#!/usr/bin/env python3
import json, os, subprocess, sys
from pathlib import Path
assert sys.argv[1:] == ['--full'] and os.environ['HIRSEL_PRIORITY'] == '40'
subprocess.run([sys.executable, 'tools/buck2/prepare.py'], check=True)
logs = Path(os.environ['HIRSEL_FULL_LOGS'])
(logs / '01-workspace-tests').mkdir(parents=True)
command = ['.buck2/bin/buck2', 'test', 'root//:a', 'root//:b']
if Path('DETAILS-gate-perf-counts').exists():
    command.append('root//:gate-perf-counts')
status = subprocess.run(command, env=os.environ | {'HIRSEL_BUCK2_LOG_DIR': str(logs / '01-workspace-tests')}).returncode
with open(logs / 'stages.jsonl', 'a') as out:
    out.write(json.dumps(dict(index=1, stage='workspace tests', logs='01-workspace-tests', tag='', command=command,
                              probe=['.buck2/bin/buck2', 'test', '{targets}'], status=status, seconds=1)) + '\n')
    out.write(json.dumps(dict(stage='native matrix', status='skipped',
                              skip=dict(stage='native matrix', reason='never completes', owner='native-coverage',
                                        date='2026-10-20'))) + '\n')
sys.exit(status)
'''
FAKE_PREPARE = '''import os
if not os.path.exists('.buck2/bin'):
    os.makedirs('.buck2', exist_ok=True)
    os.symlink(os.environ['FAKE_BUCK2_BIN'], '.buck2/bin')
'''
# sluice: a step is running when it is in RUNNING; `say` is logged with its body.
FAKE_SLUICE = r'''#!/usr/bin/env python3
import json, os, sys
args = sys.argv[1:]
value = lambda flag: args[args.index(flag) + 1]
if args[:2] == ['tool', 'step_get']:
    status = 'running' if value('--step') in os.environ.get('RUNNING', '').split() else 'done'
    print(json.dumps({'step': {'status': status}}))
elif args[:2] == ['tool', 'say']:
    with open(os.environ['SAID'], 'a') as out:
        out.write(json.dumps(dict(to=value('--to'), me=os.environ.get('SLUICE_ME'),
                                  body=open(value('--body-file')).read())) + '\n')
'''
# The pool's trace host: one usage event of the runner's lane, now.
FAKE_SSH = r'''#!/usr/bin/env python3
import json, sys, time
if 'lane:main-runs|' in sys.argv[-1]:
    print(json.dumps(dict(event='usage', operation='op', attempt='1', identity='repo:hirsel-lang|lane:main-runs|user:t',
                          effective_cpu=2, wall_time_ms=1_800_000, cpu_time_ms=1_800_000, end_s=time.time())))
'''


def git(root, *args):
    return subprocess.run(['git', *args], cwd=root, check=True, capture_output=True, text=True).stdout.strip()


class MainRuns(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        top = Path(self.temp.name)
        self.origin, self.clone, self.runs = top / 'origin.git', top / 'clone', top / 'runs'
        fake = top / 'bin'
        fake.mkdir()
        for name, text in (('buck2', FAKE_BUCK2), ('sluice', FAKE_SLUICE), ('ssh', FAKE_SSH)):
            (fake / name).write_text(text)
            (fake / name).chmod(0o755)
        self.said, self.commands = top / 'said', top / 'commands'
        self.env = os.environ | {'HIRSEL_MAIN_RUNS': str(self.runs), 'HIRSEL_SLUICE': str(fake / 'sluice'),
                                 'PATH': f'{fake}:{os.environ["PATH"]}', 'FAKE_BUCK2_BIN': str(fake),
                                 'SAID': str(self.said), 'COMMANDS': str(self.commands), 'RUNNING': 'lane-two',
                                 'HIRSEL_TEST_HISTORY': str(top / 'history.tsv')}
        for name in ('HIRSEL_MAIN_WORKTREES', 'HIRSEL_GATE_SLOTS', 'HIRSEL_BUCK2_LOG_DIR', 'HIRSEL_PRIORITY',
                     'HIRSEL_FULL_LOGS', 'HIRSEL_BUCK2_CONFIG'):
            self.env.pop(name, None)
        subprocess.run(['git', 'init', '-q', '--bare', '-b', 'main', str(self.origin)], check=True)
        subprocess.run(['git', 'clone', '-q', str(self.origin), str(self.clone)], check=True, capture_output=True)
        git(self.clone, 'config', 'user.email', 'test@example.invalid')
        git(self.clone, 'config', 'user.name', 'Main runs test')
        files = {'tools/main-runs': (TOOLS / 'main-runs').read_text(), 'tools/gate': FAKE_GATE,
                 'tools/buck2/prepare.py': FAKE_PREPARE, '.gitignore': '.buck2/\n',
                 'tools/known-failures.tsv': '# id\treason\towner\tdate\n'}
        for helper in (TOOLS / 'buck2').glob('*.py'):
            if helper.name != 'prepare.py':
                files[f'tools/buck2/{helper.name}'] = helper.read_text()
        self.commit(files, 'Base')
        for name in ('main-runs', 'gate'):
            (self.clone / 'tools' / name).chmod(0o755)
        git(self.clone, 'commit', '-qam', 'Executable tools')
        git(self.clone, 'push', '-q', 'origin', 'main')

    def commit(self, files, message):
        for path, text in files.items():
            (self.clone / path).parent.mkdir(parents=True, exist_ok=True)
            (self.clone / path).write_text(text)
        git(self.clone, 'add', '-A')
        git(self.clone, 'commit', '-qm', message)
        return git(self.clone, 'rev-parse', 'HEAD')

    def land(self, lanes):
        """Push one commit per lane, as one batch, and log its landings."""
        batch = [lane for lane, _ in lanes]
        rows = []
        for position, (lane, files) in enumerate(lanes, 1):
            tip = self.commit(files, f'{lane} lands')
            rows.append(dict(branch=lane, landed=tip, batch=batch, batch_position=position))
        git(self.clone, 'push', '-q', 'origin', 'main')
        pushed = git(self.clone, 'rev-parse', 'HEAD')
        with open(self.clone / '.git/hirsel-landings.jsonl', 'a') as out:
            out.writelines(json.dumps(row | dict(pushed=pushed)) + '\n' for row in rows)
        return [row['landed'] for row in rows]

    def tick(self):
        result = subprocess.run([sys.executable, str(self.clone / 'tools/main-runs'), '--tick'], cwd=self.clone,
                                env=self.env, capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        return result

    def rows(self, kind):
        path = self.clone / '.git/hirsel-main-runs.jsonl'
        rows = [json.loads(line) for line in path.read_text().splitlines()] if path.exists() else []
        return [row for row in rows if row['kind'] == kind]

    def messages(self):
        return [json.loads(line) for line in self.said.read_text().splitlines()] if self.said.exists() else []

    def test_a_run_records_every_target_its_cpu_and_its_skipped_stages_and_an_unmoved_main_is_skipped(self):
        self.tick()
        [run] = self.rows('run')
        self.assertEqual(run['sha'], git(self.clone, 'rev-parse', 'origin/main'))
        self.assertEqual((run['result'], run['targets']),
                         ('pass', {'a': 'pass', 'b': 'pass', 'stage: workspace tests': 'pass'}))
        self.assertEqual((run['cpu_hours'], run['cpu_used_hours']), (1.0, 0.5))
        self.assertEqual([skip['stage'] for skip in run['skipped']], ['native matrix'])
        self.tick()
        self.assertEqual(len(self.rows('run')), 1)
        self.assertIn('skip: origin/main is still', (self.runs / 'ticks.log').read_text())
        status = subprocess.run([sys.executable, str(self.clone / 'tools/main-runs'), '--status'], cwd=self.clone,
                                env=self.env, capture_output=True, text=True, check=True).stdout
        self.assertIn('reds: 0', status)
        self.assertIn('skipped stage: native matrix: never completes (owner native-coverage', status)

    def test_a_tick_while_a_run_holds_the_lock_is_skipped_not_queued(self):
        self.runs.mkdir()
        with open(self.runs / 'main-runs.lock', 'a+') as held:
            fcntl.flock(held, fcntl.LOCK_EX)
            self.tick()
        self.assertEqual(self.rows('run'), [])
        self.assertIn('skip: a run is still going', (self.runs / 'ticks.log').read_text())

    def test_a_planted_red_is_bisected_to_its_landing_listed_and_its_lane_told(self):
        self.tick()
        green = git(self.clone, 'rev-parse', 'origin/main')
        one, two, three = self.land([('lane-one', {'one.txt': '1\n'}), ('lane-two', {'RED-b': 'red\n'}),
                                     ('lane-three', {'three.txt': '3\n'})])
        self.tick()
        run = self.rows('run')[-1]
        self.assertEqual((run['sha'], run['result'], run['targets']['a'], run['targets']['b']),
                         (three, 'fail', 'pass', 'fail'))
        [red] = self.rows('red')
        self.assertEqual((red['key'], red['green'], red['culprit'], red['owner'], red['told']),
                         ('b', green, two, 'lane-two', 'lane-two'))
        self.assertEqual(red['batch'], ['lane-one', 'lane-two', 'lane-three'])
        # Only the failing target ran at each bisect point, never a.
        probes = [line for line in self.commands.read_text().splitlines() if 'main-bisect' in line]
        self.assertTrue(probes)
        self.assertTrue(all(line.endswith(' root//:b') for line in probes), probes)
        # The known failure is on main, owned by the culprit's lane.
        git(self.clone, 'fetch', '-q', 'origin')
        listed = git(self.clone, 'show', 'origin/main:tools/known-failures.tsv').splitlines()[-1].split('\t')
        self.assertEqual((listed[0], listed[2]), ('b::case_x', 'lane-two'))
        self.assertIn(two[:12], listed[1])
        [told] = self.messages()
        self.assertEqual((told['to'], told['me']), ('lane-two', '@main-runs'))
        self.assertIn(f'turned red on origin/main at {two[:12]}, the landing of lane-two', told['body'])
        self.assertIn('Repro: git checkout', told['body'])
        # Still red in the next run (main moved by the known-failure row): the orchestrator hears once.
        self.tick()
        self.assertEqual(len(self.rows('run')), 3)
        self.assertEqual(len(self.rows('red')), 1)
        escalated = self.messages()[1:]
        self.assertEqual([message['to'] for message in escalated], ['@agentlang-24'])
        self.assertIn('survived a second run', escalated[0]['body'])
        status = subprocess.run([sys.executable, str(self.clone / 'tools/main-runs'), '--status'], cwd=self.clone,
                                env=self.env, capture_output=True, text=True, check=True).stdout
        self.assertIn(f'  b: red for 2 runs; owner lane-two, culprit {two[:12]}', status)

    def test_a_passing_perf_run_ratchets_the_lower_budgets_onto_main_and_flags_a_large_drop(self):
        def row(total, function):
            return {'Ir': total, 'functions': {'f': function}}
        before = {'micro/a': row(1000, 900), 'micro/b': row(1000, 900)}
        table = runpy.run_path(str(TOOLS / 'perf-counts'))['table']
        # micro/a drops 20% (ratcheted and checked by hand); micro/b's 0.5% is noise.
        measured = {'micro/a': row(800, 700), 'micro/b': row(995, 900)}
        record = 'PERF_COUNTS_RECORD ' + json.dumps(measured, sort_keys=True, separators=(',', ':'))
        sha = self.commit({'tools/perf-counts': (TOOLS / 'perf-counts').read_text(),
                           'tools/perf-counts.tsv': table(before),
                           'DETAILS-gate-perf-counts': json.dumps(record)[1:-1]}, 'Perf baseline')
        git(self.clone, 'push', '-q', 'origin', 'main')
        self.tick()
        [run] = self.rows('run')
        self.assertEqual((run['sha'], run['targets']['gate-perf-counts']), (sha, 'pass'))
        self.assertEqual(run['perf']['improvements'], [dict(count='micro/a', old=1000, new=800)])
        self.assertEqual(run['perf']['checks'], ['micro/a'])
        git(self.clone, 'fetch', '-q', 'origin')
        self.assertEqual(git(self.clone, 'rev-parse', 'origin/main'), run['perf']['pushed'])
        self.assertEqual(git(self.clone, 'log', '-1', '--format=%s', 'origin/main'),
                         f'Ratchet the instruction budgets main measured at {sha[:12]}.')
        self.assertEqual(git(self.clone, 'show', 'origin/main:tools/perf-counts.tsv') + '\n',
                         table({'micro/a': row(800, 700), 'micro/b': row(1000, 900)}))
        [told] = self.messages()
        self.assertEqual(told['to'], '@agentlang-24')
        self.assertIn('PERF-CHECK', told['body'])
        self.assertIn('micro/a 1,000 -> 800', told['body'])
        # The ratchet's push is a new main measuring the same: nothing more to write.
        self.tick()
        self.assertEqual(self.rows('run')[-1]['perf'], None)
        self.assertEqual(len(self.messages()), 1)

    def test_a_red_whose_lane_has_finished_goes_to_the_orchestrator(self):
        self.env['RUNNING'] = ''
        self.tick()
        _, two = self.land([('lane-one', {'one.txt': '1\n'}), ('lane-two', {'RED-a': 'red\n'})])
        self.tick()
        [red] = self.rows('red')
        self.assertEqual((red['culprit'], red['owner'], red['told']), (two, 'lane-two', '@agentlang-24'))

    def test_reds_with_no_earlier_passing_run_go_to_the_orchestrator_unbisected(self):
        self.commit({'RED-a': 'red\n'}, 'Red from the start')
        git(self.clone, 'push', '-q', 'origin', 'main')
        self.tick()
        [red] = self.rows('red')
        self.assertEqual((red['key'], red['culprit']), ('a', None))
        [told] = self.messages()
        self.assertEqual(told['to'], '@agentlang-24')
        self.assertIn('no earlier passing run', told['body'])
        self.assertFalse(any('main-bisect' in line for line in self.commands.read_text().splitlines()))


class GateCommands(unittest.TestCase):
    """`tools/gate --full` with a fake Buck2 and fake helpers."""

    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        for directory in ('.buck2/bin', 'tools/tests', 'tools/buck2', 'bench/native/tests'):
            (self.root / directory).mkdir(parents=True)
        for test in ('main_runs', 'buck2_sync', 'gate'):
            (self.root / f'tools/tests/{test}.py').write_text('# fixture gate tests\n')
        (self.root / 'bench/native/tests/test_fixture.py').write_text(
            'import unittest\nclass Fixture(unittest.TestCase):\n    def test_fixture(self):\n        pass\n')
        for helper in ('gating', 'generated', 'graph', 'impact', 'event_logs', 'test_sizes_from_usage'):
            (self.root / f'tools/buck2/{helper}.py').write_text((TOOLS / f'buck2/{helper}.py').read_text())
        for helper in ('prepare', 'sync', 'clippy', 'fmt', 'conformance', 'agree'):
            (self.root / f'tools/buck2/{helper}.py').write_text(f'''import os, sys
AGREE_LIMIT = {AGREE_LIMIT}
AGREE_SHARDS = CONFORMANCE_SHARDS = 16
AGREE_SLICE_PROGRAMS = CONFORMANCE_SLICE_PROGRAMS = 300
SELECTED_SLICES = 16
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
        gate.write_text((TOOLS / 'gate').read_text())
        gate.chmod(0o755)
        self.commands = self.root / 'commands'
        self.env = os.environ | {'COMMANDS': str(self.commands), 'HIRSEL_TEST_SEED': '7',
                                 'HIRSEL_BUCK2_LOG_DIR': str(self.root / '.buck2/event-logs')}
        for name in ('HIRSEL_PRIORITY', 'HIRSEL_FULL_LOGS', 'HIRSEL_BUCK2_CONFIG'):
            self.env.pop(name, None)
        buck2 = self.root / '.buck2/bin/buck2'
        buck2.write_text('''#!/usr/bin/env bash
if [[ ${2:-} == --event-log ]]; then command=$1; shift 3; set -- "$command" "$@"; fi
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

    def test_tool_tests_run_after_the_build_files_without_the_runs_settings(self):
        # A tool test runs fake full gates: the run's log directory and priority would leak into them.
        (self.root / 'tools/tests/gate.py').write_text(
            'import os\nwith open(os.environ["COMMANDS"], "a") as log:\n'
            '    print("tool test: " + " ".join(n for n in ("HIRSEL_FULL_LOGS", "HIRSEL_PRIORITY", "HIRSEL_BUCK2_CONFIG",'
            ' "HIRSEL_BUCK2_LOG_DIR", "HIRSEL_TEST_SEED") if n in os.environ), file=log)\n')
        self.env |= {'HIRSEL_PRIORITY': '40', 'HIRSEL_FULL_LOGS': str(self.root / 'logs')}
        result = self.run_full()
        self.assertEqual(result.returncode, 0, result.stderr)
        commands = self.commands.read_text().splitlines()
        self.assertIn('tool test: ', commands)
        self.assertLess(commands.index('sync.py --check'), commands.index('tool test: '))
        stages = [json.loads(line)['stage'] for line in (self.root / 'logs/stages.jsonl').read_text().splitlines()]
        self.assertEqual(stages.count('tool tests: tools/tests/gate.py'), 1)

    def test_full_keeps_unrestricted_campaigns_and_all_old_gates(self):
        result = self.run_full()
        self.assertEqual(result.returncode, 0, result.stderr)
        commands = self.commands.read_text().splitlines()
        full = 'test -c hirsel.full=true -c hirsel.test_seed=7 '
        for command in ['prepare.py', 'sync.py --check', 'clippy.py', 'fmt.py --check', 'conformance.py', 'agree.py --full',
                        'agree.py --native-full',
                        'build -c hirsel.full=true -c hirsel.test_seed=7 //: --materializations none', 'native gate',
                        full + 'root//:a-unittest root//:b-test-it root//:doclib -- --timeout 1800',
                        full + '--target-platforms //tools/buck2:release root//:a-unittest root//:b-test-it root//:doclib -- --timeout 1800',
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
        self.env = os.environ | {'COMMANDS': str(self.commands),
                                 'HIRSEL_BUCK2_LOG_DIR': str(self.root / '.buck2/event-logs')}
        (self.root / 'tools').mkdir()
        (self.root / 'tools/check-native.sh').write_text((TOOLS / 'check-native.sh').read_text())
        (self.root / 'tools/buck2').mkdir()
        (self.root / 'tools/buck2/event_logs.py').write_text((TOOLS / 'buck2/event_logs.py').read_text())
        for tool in ('.buck2/bin/buck2', 'proofs/rc/audit.sh', 'proofs/rc/check-vectors.sh',
                     'proofs/ordered/check.sh'):
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
        result = self.run_native('gate-native-runtime-c')
        self.assertEqual(result.returncode, 7)
        commands = self.commands.read_text().splitlines()
        self.assertIn('check-native.sh: runtime C harness failed (exit 7,', result.stderr)
        self.assertIn('check-native.sh: library C probes passed', result.stdout)
        self.assertIn('check-native.sh: 1 failing stages: runtime C harness\n', result.stderr)
        # Independent Buck2 harnesses and every later stage still run.
        for target in ('gate-native-library-c', 'gate-native-toolchain-pin'):
            self.assertTrue(any(c.startswith('buck2 test') and target in c for c in commands), target)
        for tool in ('audit.sh', 'check-vectors.sh', 'check.sh'):
            self.assertTrue(any(c.startswith(tool) for c in commands), tool)
        self.assertIn('check-native.sh: Lean RC model vectors passed', result.stdout)
        self.assertTrue(commands[-1].startswith('buck2 test') and 'rc_maps_native::' in commands[-1])

    def test_a_clean_run_passes(self):
        result = self.run_native('no-failing-command')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn('check-native.sh: passed', result.stdout)
        commands = self.commands.read_text().splitlines()
        for target in ('gate-native-runtime-c', 'gate-native-library-c', 'gate-native-toolchain-pin'):
            self.assertTrue(any(c.startswith('buck2 test') and target in c for c in commands), target)
        self.assertFalse(any('hirsel.tests=host' in c for c in commands))


class AgreementShards(unittest.TestCase):
    def setUp(self):
        sys.path.insert(0, str(TOOLS / 'buck2'))
        import agree
        self.agree = agree
        self.report = dict(seed=1, limit=4, hash_seed=0, programs=3, generated=9, eligible=9, variants_per_program=96,
                           checked=2, skipped=0, rejected=1, valid=1, unavailable=0,
                           runtime_limits=0, executed=1, edits={'mode': 2}, executions={'fail': 1},
                           clusters=[])

    def outputs(self):
        return {f'shard-{i}': '---- STDOUT ----\n' + json.dumps(
            self.report | {'shard': dict(index=i, count=2)}, indent=2) for i in (1, 2)}

    def test_sums_classifications_and_edits_but_keeps_global_catalogue_counts(self):
        result = self.agree.summarize(self.outputs(), 2, 4)
        self.assertEqual(result, self.report | dict(checked=4, rejected=2, valid=2,
                                                   executed=2, edits={'mode': 4},
                                                   executions={'fail': 2}))

    def test_refuses_missing_duplicate_mismatched_and_flagged_shards(self):
        for changed in [
                {'shard': dict(index=1, count=2)},
                {'shard': dict(index=2, count=3)},
                {'generated': 10}, {'eligible': 10}, {'variants_per_program': 64}, {'checked': 1}, {'edits': {}},
                {'executions': {'run': 1, 'fail': 1}},
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


    def test_slices_of_chosen_cases_sum_and_refuse_a_flag_or_another_sample(self):
        slices = {f'slice-{i}': json.dumps(self.report | dict(limit=self.agree.AGREE_LIMIT)) for i in (1, 2)}
        result = self.agree.summarize_within(slices)
        self.assertEqual((result['checked'], result['valid'], result['generated']), (4, 2, 9))
        for changed in [{'seed': 2}, {'generated': 10}, {'clusters': [dict(kind='bug', path='x.hir')]},
                        {'rejected': 2}]:
            with self.subTest(changed=changed):
                bad = dict(slices, **{'slice-2': json.dumps(self.report | dict(limit=self.agree.AGREE_LIMIT) | changed)})
                with self.assertRaises(ValueError):
                    self.agree.summarize_within(bad)
        with self.assertRaisesRegex(ValueError, 'no agreement slice'):
            self.agree.summarize_within({})

    def test_the_unrestricted_campaign_covers_the_whole_catalogue(self):
        self.report |= dict(limit=self.agree.UNLIMITED, generated=4, eligible=4)
        result = self.agree.summarize(self.outputs(), 2, self.agree.UNLIMITED)
        self.assertEqual(result['checked'], 4)
        with self.assertRaisesRegex(ValueError, 'sample parameters'):
            self.agree.summarize(self.outputs(), 2, 4)
        self.report |= dict(generated=5, eligible=5)
        with self.assertRaisesRegex(ValueError, 'did not cover'):
            self.agree.summarize(self.outputs(), 2, self.agree.UNLIMITED)


class NativeAgreementShards(unittest.TestCase):
    def setUp(self):
        sys.path.insert(0, str(TOOLS / 'buck2'))
        import agree
        self.agree = agree
        self.known = {'native/heap/a', 'native/llvm/b'}

    def report(self, index, *kinds, **changed):
        clusters = [dict(kind=kind, edit='mode', count=1, path=f'{kind}.hir', source='reduced', detail='')
                    for kind in kinds]
        return '---- STDOUT ----\n' + json.dumps(dict(
            seed=1, limit=self.agree.UNLIMITED, hash_seed=0, programs=2, generated=9, eligible=7,
            variants_per_program=96, checked=3, skipped=index - 1, valid=2, executed=2,
            native_agreed=4, clusters=clusters, shard=dict(index=index, count=2)) | changed, indent=2)

    def streamed(self, *kinds):
        lines = ['---- STDERR ----', 'agree: variants: x.hir mode 1..2 running for 30 s']
        lines += ['agree: cluster ' + json.dumps(dict(kind=kind, edit='mode', path=f'{kind}.hir',
                                                      source='unreduced', detail='')) for kind in kinds]
        return '\n'.join(lines)

    def test_unions_complete_shards_and_judges_the_listed_kinds(self):
        merged, errors = self.agree.merge_native(
            {'shard-1': self.report(1, 'native/heap/a'), 'shard-2': self.report(2, 'native/llvm/b')},
            2, self.known)
        self.assertEqual(errors, [])
        self.assertEqual([c['kind'] for c in merged['clusters']], ['native/heap/a', 'native/llvm/b'])
        self.assertEqual((merged['checked'], merged['skipped'], merged['eligible'], merged['complete'],
                          merged['known_kinds']), (6, 1, 7, 2, 2))

    def test_shards_share_a_rotated_seed_and_its_cover(self):
        merged, errors = self.agree.merge_native(
            {'shard-1': self.report(1, seed=7), 'shard-2': self.report(2, seed=7)}, 2, set())
        self.assertEqual((errors, merged['seed']), ([], 7))
        for changed in [dict(seed=8), dict(eligible=8), dict(programs=3), dict(variants_per_program=64)]:
            with self.subTest(changed=changed):
                _, errors = self.agree.merge_native(
                    {'shard-1': self.report(1, seed=7), 'shard-2': self.report(2, **dict(seed=7) | changed)},
                    2, set())
                self.assertIn('shard-2: agreement parameters differ from the gate', errors)

    def test_complete_shards_must_check_or_skip_every_eligible_variant(self):
        for skipped, eligible in [(0, 7), (2, 7), (1, 8)]:
            with self.subTest(skipped=skipped, eligible=eligible):
                _, errors = self.agree.merge_native(
                    {'shard-1': self.report(1, eligible=eligible),
                     'shard-2': self.report(2, skipped=skipped, eligible=eligible)}, 2, set())
                self.assertEqual(errors, [f'native agreement shards checked and skipped {6 + skipped} '
                                          f'of {eligible} eligible variants'])

    def test_a_new_kind_and_a_kind_no_longer_seen_fail(self):
        _, errors = self.agree.merge_native(
            {'shard-1': self.report(1, 'native/heap/a', 'native/heap/new'), 'shard-2': self.report(2)},
            2, self.known)
        self.assertEqual(errors, ['new discrepancy kind: native/heap/new',
                                  'native/llvm/b no longer occurs; delete it from '
                                  'native-tests/regressions/agree-known.txt'])

    def test_a_stopped_shard_keeps_its_streamed_clusters_but_judges_no_absence(self):
        merged, errors = self.agree.merge_native(
            {'shard-1': self.report(1), 'shard-2': self.streamed('native/heap/a', 'native/heap/new')},
            2, self.known)
        self.assertEqual([c['source'] for c in merged['clusters']], ['unreduced', 'unreduced'])
        self.assertEqual(errors, ['shard-2 stopped before its report; its streamed clusters are kept',
                                  '1 of 2 native agreement shards reported; missing [2]',
                                  'new discrepancy kind: native/heap/new'])

    def test_refuses_a_duplicate_shard_and_other_parameters(self):
        _, errors = self.agree.merge_native(
            {'shard-1': self.report(1), 'shard-2': self.report(1)}, 2, set())
        self.assertIn("shard-2: duplicate or invalid native agreement shard: {'index': 1, 'count': 2}", errors)
        limited = self.report(2).replace(str(self.agree.UNLIMITED), '5000')
        _, errors = self.agree.merge_native({'shard-1': self.report(1), 'shard-2': limited}, 2, set())
        self.assertIn('shard-2: agreement parameters differ from the gate', errors)

    def test_known_kinds_skip_comments_and_blank_lines(self):
        self.assertEqual(self.agree.known_kinds('# header\n\nnative/heap/a\n'), {'native/heap/a'})


class ActionSizing(unittest.TestCase):
    def setUp(self):
        sys.path.insert(0, str(TOOLS / 'buck2'))
        import action_sizes_from_log
        self.module = action_sizes_from_log
        self.action = dict(target='hirsel-check', profile='dev', phase='rlib [pic]',
                           wall=10., end=100., cpu=8, memory_kb=1048576)
        self.record = dict(end=100, requested_cpu='8', requested_kb='1048576',
                           wall_ms='10000', cpu_usec='43000000', peak_bytes='900000000',
                           anon_peak_bytes='600000000', worker='fixture')

    def test_optimized_test_tools_keep_the_dev_compile_budget(self):
        from unittest.mock import patch
        event = {'Event': {'data': {'SpanEnd': {'data': {'ActionExecution': {
            'name': {'category': 'rustc', 'identifier': 'rlib [pic]'},
            'failed': False,
            'key': {'owner': {'TargetLabel': {
                'label': {'package': 'root//', 'name': 'hirsel-check'},
                'configuration': {'full_name': 'root//tools/buck2:checked#fixture'},
            }}},
            'commands': [{'details': {
                'command_kind': {'command': {'RemoteCommand': {
                    'cache_hit': False,
                    'details': {'platform': {'properties': [
                        {'name': 'cpu_count', 'value': '1'},
                        {'name': 'memory_kb', 'value': '1048576'},
                    ]}},
                }}},
                'metadata': {'execution_time_us': 10000000, 'start_time': [90, 0]},
            }}],
        }}}}}}
        with patch.object(self.module.event_logs, 'lines', return_value=[json.dumps(event)]):
            actions = list(self.module.events('fixture'))
        self.assertEqual(len(actions), 1)
        self.assertEqual(actions[0]['profile'], 'dev')
        self.assertEqual(actions[0]['memory_kb'], 1048576)

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
