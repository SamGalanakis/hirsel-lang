#!/usr/bin/env python3
"""tools/main-runs names each red's likely culprit and lane from the commits
since the last run of the red's stage, tells each lane its new reds once, and
sends a red with no known lane to the orchestrator; a fast tier runs only the
quick stages."""
import json
import os
from pathlib import Path
import runpy
import signal
import sys
import shutil
import subprocess
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]


class NativeCollection(unittest.TestCase):
    def test_whole_native_stage_explicitly_requests_the_shared_cover(self):
        stages = runpy.run_path(str(ROOT / 'tools/main-runs'))['stages']
        commands = {}
        with patch.dict(os.environ), patch('subprocess.run') as run:
            run.return_value.stdout = 'root//:example-unittest'
            stages(lambda name, command, **kwargs: commands.update({name: command}), 'full')
        native = commands['native matrix: both targets and named exclusions']
        at = native.index('hirsel.native_selection=true')
        self.assertEqual(native[at - 1], '-c')
        self.assertIn('//:gate-native-full-shard-1', native)


class Route(unittest.TestCase):
    def setUp(self):
        self.repo = Path(tempfile.mkdtemp(prefix='main-runs-test-'))
        self.addCleanup(shutil.rmtree, self.repo)
        self.env = {k: v for k, v in os.environ.items() if not k.startswith('GIT_')} | dict(
            GIT_AUTHOR_NAME='t', GIT_AUTHOR_EMAIL='t@t', GIT_COMMITTER_NAME='t', GIT_COMMITTER_EMAIL='t@t')
        self.git('init', '-q', '-b', 'main')
        self.commits = {}
        landings = []
        # lane-a lands two commits under hirsel-foo; lane-b one under hirsel-bar and the corpus.
        for name, lane, paths in (('base', None, ['README.md']),
                                  ('foo-1', None, ['crates/hirsel-foo/src/a.rs']),
                                  ('foo-2', 'lane-a', ['crates/hirsel-foo/src/b.rs', 'docs/x.md']),
                                  ('bar', 'lane-b', ['crates/hirsel-bar/src/c.rs', 'conformance/SEM/SEM-1-x-run.hir'])):
            for path in paths:
                (self.repo / path).parent.mkdir(parents=True, exist_ok=True)
                (self.repo / path).write_text(name + '\n')
            self.git('add', '-A')
            self.git('commit', '-qm', name)
            self.commits[name] = self.git('rev-parse', 'HEAD')
            if lane:
                landings.append(f'{{"landed": "{self.commits[name]}", "branch": "{lane}"}}\n')
        (self.repo / 'crates/hirsel-foo-macros').mkdir()
        (self.repo / '.git/hirsel-landings.jsonl').write_text(''.join(landings))
        self.said = self.repo / 'said.txt'
        sluice = self.repo / 'sluice'
        sluice.write_text(f'#!/bin/sh\n[ "$6" = lane-gone ] && exit 1\n'
                          f'{{ echo "TO $6"; cat "$8"; }} >> {self.said}\n')
        sluice.chmod(0o755)
        self.route = runpy.run_path(str(ROOT / 'tools/main-runs'))['route']
        self.route.__globals__.update(TREE=self.repo, SLUICE=str(sluice))

    def git(self, *arguments):
        return subprocess.run(['git', *arguments], cwd=self.repo, env=self.env, check=True, capture_output=True,
                              text=True).stdout.strip()

    def test_each_red_goes_to_the_lane_whose_commits_match_it(self):
        reds = {
            'workspace tests: hirsel-foo-test-it-shard-2': dict(stage='workspace tests', target='hirsel-foo-test-it-shard-2',
                                                                ids=['hirsel-foo-test-it::walk']),
            'conformance validation and ratchet: gate-conformance-shard-3': dict(
                stage='conformance validation and ratchet', target='gate-conformance-shard-3', ids=['conformance/SEM/SEM-1-x-run.hir']),
            'stage: native and Lean checks': dict(stage='native and Lean checks', target=None, ids=[]),
            'workspace tests: hirsel-bar-unittest': dict(stage='workspace tests', target='hirsel-bar-unittest', ids=[]),
        }
        # The full run before has the old red; the fast run since ran only workspace tests and had it too.
        old_red = dict(culprit='c0ffee', lane='lane-z', since='5eed')
        history = [dict(sha=self.commits['base'], reds={'workspace tests: hirsel-bar-unittest': old_red}),
                   dict(sha=self.commits['foo-2'], tier='fast', stages=['workspace tests'],
                        reds={'workspace tests: hirsel-bar-unittest': old_red})]
        self.route(self.commits['bar'], reds, history, '/report.md')
        foo, corpus, stage, old = reds.values()
        # A red in a stage the fast run ran is new since that run, so only bar can be its culprit;
        # the corpus red's stage last ran in the full run, so its culprit is among all three commits.
        self.assertEqual((foo['culprit'], foo['lane']), (self.commits['bar'], 'lane-b'))
        self.assertEqual((corpus['culprit'], corpus['lane']), (self.commits['bar'], 'lane-b'))
        self.assertEqual((stage['culprit'], stage['lane']), (None, None))
        self.assertEqual((old['culprit'], old['lane'], old['since']), ('c0ffee', 'lane-z', '5eed'))
        said = self.said.read_text()
        self.assertEqual(said.count('TO '), 2)
        for lane, key in (('lane-b', 'hirsel-foo-test-it-shard-2'), ('lane-b', 'gate-conformance-shard-3'),
                          ('@agentlang-24', 'native and Lean checks')):
            message = said.split(f'TO {lane}\n')[1].split('TO ')[0]
            self.assertIn(key, message)
        self.assertNotIn('hirsel-bar-unittest', said)

    def test_exit_71_stage_is_not_routed_to_a_lane(self):
        # Only lane-b landed since the preceding run: a generic stage failure
        # would be blamed on it. Exercise stage execution through notification.
        namespace = self.route.__globals__
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            with patch.dict(namespace, stages=lambda stage, tier: stage(
                    'memory guard', [namespace['PY'], '-c', 'raise SystemExit(71)'])):
                with (directory / 'run.log').open('w+') as log:
                    reds, *_ = namespace['run_stages'](directory, log, 'fast')
            self.route(self.commits['bar'], reds,
                       [dict(sha=self.commits['foo-2'], reds={})], '/report.md')
        said = self.said.read_text()
        self.assertNotIn('TO lane-b', said)
        self.assertIn('TO @agentlang-24', said)
        self.assertIsNone(reds['stage: memory guard']['culprit'])
        self.assertIsNone(reds['stage: memory guard']['lane'])

    def test_infra_is_batched_without_reusing_old_lane_blame(self):
        reds = {f'workspace tests: {name}': dict(stage='workspace tests', target=name, ids=[],
                                               kind='infra', reason=reason)
                for name, reason in (('oom', 'memory limit'), ('slow', 'timeout'))}
        history = [dict(sha=self.commits['foo-2'], reds={
            key: dict(culprit='c0ffee', lane='lane-a', since='5eed') for key in reds})]
        self.route(self.commits['bar'], reds, history, '/report.md')
        said = self.said.read_text()
        self.assertEqual(said.count('TO '), 1)
        self.assertIn('TO @agentlang-24', said)
        self.assertIn('memory limit', said)
        self.assertIn('timeout', said)
        self.assertNotIn('lane-a', said)
        self.assertEqual(sum(line.startswith('- ') for line in said.splitlines()), 2)
        for red in reds.values():
            self.assertIsNone(red['culprit'])
            self.assertIsNone(red['lane'])
        # Infra is batched each run; a code failure replacing infra is new.
        self.said.unlink()
        self.route(self.commits['bar'], reds, [dict(sha=self.commits['bar'], reds={})], '/report.md')
        self.assertEqual(self.said.read_text().count('TO @agentlang-24'), 1)
        code = {'workspace tests: oom': dict(stage='workspace tests', target='oom', ids=[], kind='code')}
        self.route(self.commits['bar'], code, [dict(sha=self.commits['foo-2'], reds={})], '/report.md')
        self.assertIn('TO lane-b', self.said.read_text())

    def test_a_lane_that_cannot_be_told_falls_back_to_the_orchestrator(self):
        landings = self.repo / '.git/hirsel-landings.jsonl'
        landings.write_text(landings.read_text().replace('lane-b', 'lane-gone'))
        reds = {'x: hirsel-bar-unittest': dict(stage='x', target='hirsel-bar-unittest', ids=[])}
        self.route(self.commits['bar'], reds, [dict(sha=self.commits['foo-2'], reds={})], '/report.md')
        self.assertEqual(reds['x: hirsel-bar-unittest']['lane'], 'lane-gone')
        self.assertIn('TO @agentlang-24\n', self.said.read_text())


class Tiers(unittest.TestCase):
    def test_fast_runs_only_the_quick_stages_and_full_runs_them_too(self):
        tool = runpy.run_path(str(ROOT / 'tools/main-runs'))
        named = {}
        for tier in ('fast', 'full'):
            named[tier] = []
            env = dict(os.environ)
            try:
                with patch('subprocess.run', return_value=subprocess.CompletedProcess([], 0, '', '')):
                    tool['stages'](lambda name, *_, **__: named[tier].append(name), tier)
            finally:
                os.environ.clear()
                os.environ.update(env)
        fast = [n for n in named['fast'] if not n.startswith('tool tests: ')]
        self.assertEqual(fast, ['Buck2 toolchains and pool configuration', 'Buck2 build files and test-cost coverage',
                                'workspace build', 'workspace clippy', 'formatting', 'workspace tests'])
        self.assertEqual(named['full'][:len(named['fast'])], named['fast'])
        self.assertIn('native passing corpus with independent second hash seed', named['full'])
        # A fast tick is skipped by a full run of main; a full tick is not skipped by a fast run.
        covers = tool['covers']
        self.assertTrue(covers({}, 'fast') and covers({'tier': 'full'}, 'fast') and covers({}, 'full'))
        self.assertFalse(covers({'tier': 'fast'}, 'full'))


class Reds(unittest.TestCase):
    def test_stage_failures_and_test_results_keep_code_and_infra_separate(self):
        tool = runpy.run_path(str(ROOT / 'tools/main-runs'))
        events = tool['event_logs']
        namespace = tool['run_stages'].__globals__
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            def stages(stage, tier):
                commands = {
                    'guard': 'raise SystemExit(71)',
                    'signal': 'import os, signal; os.kill(os.getpid(), signal.SIGTERM)',
                    'shell signal': 'raise SystemExit(137)',
                    'timeout': 'raise SystemExit(124)',
                    'capped': 'print("capped: memory limit 4G reached (HIRSEL_MEMORY_LIMIT): the kernel killed the command"); raise SystemExit(1)',
                    'buck infra': 'print("Infra Failure: unavailable"); raise SystemExit(1)',
                    'remote': 'print("Remote execution error: unavailable"); raise SystemExit(1)',
                    'pool': 'print("pool is down"); raise SystemExit(1)',
                    'code': 'print("Tests finished: Pass 4. Fail 1. Timeout 0. Fatal 0. Infra Failure 0. Build failure 0"); raise SystemExit(1)',
                    'pass': 'print("Infra Failure from an earlier retry")',
                    'mixed': 'print("Infra Failure: lost remote worker"); raise SystemExit(1)',
                    'following code': 'raise SystemExit(1)',
                }
                for name, script in commands.items():
                    if name == 'mixed':
                        logs = directory / 'logs/mixed'
                        logs.mkdir(parents=True)
                        (logs / 'test.pb.zst').touch()
                    stage(name, [sys.executable, '-c', script])
            results = {
                'hirsel-foo': dict(status=events.FAIL, details='test walk ... FAILED'),
                'slow': dict(status=events.TIMED_OUT, details=''),
                'guard': dict(status=events.FAIL, details='test process exited with code 71'),
                'fatal': dict(status=events.FATAL, details='Remote execution error: lost worker'),
                'buck infra': dict(status=events.INFRA_FAILURE, details='worker vanished'),
                'known': dict(status=events.FAIL, details='assertion failed'),
            }
            with patch.dict(namespace, stages=stages, table=lambda path: {'known': [], 'slow': []}), \
                    patch.object(events, 'test_results', return_value=results):
                with (directory / 'run.log').open('w') as log:
                    reds, ran, *_ = tool['run_stages'](directory, log, 'fast')
        for name in ('guard', 'signal', 'shell signal', 'timeout', 'capped', 'buck infra', 'remote', 'pool'):
            self.assertEqual(reds[f'stage: {name}']['kind'], 'infra', name)
        for name in ('code', 'following code'):
            self.assertEqual(reds[f'stage: {name}']['kind'], 'code', name)
        self.assertNotIn('stage: pass', reds)
        self.assertNotIn('mixed: known', reds)
        self.assertEqual(reds['mixed: hirsel-foo']['kind'], 'code')
        for name in ('slow', 'guard', 'fatal', 'buck infra'):
            self.assertEqual(reds[f'mixed: {name}']['kind'], 'infra', name)
        self.assertEqual(len(ran), 12)

    def test_report_separates_infra_from_code(self):
        tool = runpy.run_path(str(ROOT / 'tools/main-runs'))
        red = dict(stage='x', target=None, ids=[], culprit=None, lane=None, since='abc')
        reds = {'stage: code': red | dict(kind='code'),
                'stage: memory': red | dict(kind='infra', reason='memory limit (exit 71)')}
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            with patch.dict(tool['run'].__globals__, RUNS=root, ROWS=root / 'runs.jsonl',
                            run_stages=lambda *args: (reds, ['x'], [], None),
                            route=lambda *args: None, rows=lambda: [], push_to_main=lambda *args: None):
                tool['run']('abc', 'fast')
            history = json.loads((root / 'runs.jsonl').read_text())
            self.assertEqual(history['reds'], {'stage: code': red})
            report = (root / 'latest.md').read_text()
            self.assertIn('1 code', report)
            self.assertIn('1 infra', report)
            code, infra = report.split('## Infrastructure reds')
            self.assertIn('stage: code', code)
            self.assertNotIn('stage: memory', code)
            self.assertIn('memory limit (exit 71)', infra)
            self.assertNotIn('culprit', infra)

    def test_failing_ids_name_libtest_cases_once_and_corpus_cases_by_path(self):
        failing_ids = runpy.run_path(str(ROOT / 'tools/main-runs'))['failing_ids']
        # Parallel libtest output can split a case's `... FAILED` line; its
        # `---- stdout ----` block still names it.
        details = ('test a::ok ... ok\n'
                   'test a::first ... FAILED\n'
                   'test a::split ... test a::other ... ok\nFAILED\n'
                   '---- a::first stdout ----\npanicked\n'
                   '---- a::split stdout ----\npanicked\n'
                   'conformance/MEM/MEM-7-x-run.hir: expected run, got reject\n'
                   '{"path": "conformance/DROP/DROP-2-y-run.hir", "verdict": "mismatch"}\n')
        self.assertEqual(failing_ids('hirsel-foo-test-it-shard-3', details),
                         ['hirsel-foo-test-it::a::first', 'hirsel-foo-test-it::a::split',
                          'conformance/MEM/MEM-7-x-run.hir', 'conformance/DROP/DROP-2-y-run.hir'])


class InfraDiagnostics(unittest.TestCase):
    def test_exit_and_diagnostic_conventions(self):
        sys.path.insert(0, str(ROOT / 'tools/buck2'))
        from run_failures import infra_reason
        for status, details in ((71, ''), (124, ''), (-signal.SIGTERM, ''), (137, ''),
                                (0, 'exit code: 71'), (0, 'process exited with code 71'),
                                (0, 'capped: peak 4096 MiB; memory limit 4G reached (HIRSEL_MEMORY_LIMIT)'),
                                (0, 'timed out after 30 s'), (0, 'terminated by signal 9'),
                                (0, 'Infra Failure'), (0, 'RE error: unavailable'),
                                (0, 'RemoteExecutionError: unavailable'),
                                (0, 'remote-execution pool unavailable'), (0, 'connection refused')):
            with self.subTest(status=status, details=details):
                self.assertIsNotNone(infra_reason(status, details))
        for status, details in ((1, 'assertion failed'), (2, 'syntax error'),
                                (0, 'test memory_limit ... FAILED'), (0, 'test timeout_case ... FAILED'),
                                (0, 'test pool_shutdown ... FAILED'), (0, 'test remote_execution_error ... FAILED'),
                                (0, 'Tests finished: Pass 4. Fail 1. Timeout 0. Fatal 0. Infra Failure 0. Build failure 0'),
                                (0, 'Infra Failure \x1b[32m0\x1b[39m')):
            with self.subTest(status=status, details=details):
                self.assertIsNone(infra_reason(status, details))


if __name__ == '__main__':
    unittest.main()
