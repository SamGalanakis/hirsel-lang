#!/usr/bin/env python3
"""tools/main-runs names each red's likely culprit and lane from the commits
since the last run of the red's stage and reports owners of new code reds;
a fast tier runs only the quick stages."""
import json
import contextlib
import io
import os
from pathlib import Path
import runpy
import signal
import sys
import shutil
import subprocess
import tempfile
import threading
import tracemalloc
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]


def empty_status():
    return dict(version=1, tables={}, known=[], observations=[], selections=[])


class ExceptionTables(unittest.TestCase):
    def setUp(self):
        self.tool = runpy.run_path(str(ROOT / 'tools/main-runs'))

    def test_valid_comments_and_all_malformed_rows(self):
        valid = 'binary::case\treason\tlane\t2026-10-21\n'
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / 'known.tsv'
            path.write_text('# comment\n\n' + valid)
            self.assertEqual(self.tool['table'](path), {'binary::case': valid.strip().split('\t')})
            controls = [(valid.replace('reason', ''), 'empty required field 2'),
                        ('binary\treason\tlane\n', 'expected 4 tab-separated columns, got 3'),
                        (valid.rstrip() + '\textra\n', 'expected 4 tab-separated columns, got 5'),
                        (valid + valid, 'duplicate identity'),
                        (valid.replace('2026-10-21', '2026-02-30'), 'invalid date'),
                        (valid.replace('2026-10-21', '2026-2-03'), 'date must be YYYY-MM-DD')]
            for column in (0, 2, 3):
                fields = valid.rstrip('\n').split('\t')
                fields[column] = ' '
                controls.append(('\t'.join(fields), f'empty required field {column + 1}'))
            for text, reason in controls:
                with self.subTest(reason=reason):
                    path.write_text('# comment\n' + text)
                    with self.assertRaises(self.tool['TableError']) as error:
                        self.tool['table'](path)
                    self.assertIn(f'{path}:', str(error.exception))
                    self.assertIn(reason, str(error.exception))
            path.write_text(valid + valid)
            with self.assertRaisesRegex(self.tool['TableError'], r'known.tsv:2: duplicate'):
                self.tool['table'](path)

    def test_unknown_skip_stage_is_rejected_even_when_out_of_tier(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / 'tools').mkdir()
            (root / 'tools/known-failures.tsv').write_text('full-only::case\twhy\tlane\t2026-10-21\n')
            skip = root / 'tools/main-runs-skip.tsv'
            skip.write_text('typo\twhy\tlane\t2026-10-21\n')
            with patch.dict(self.tool['exception_tables'].__globals__, TREE=root):
                with self.assertRaisesRegex(self.tool['TableError'], r'main-runs-skip.tsv:1: unknown skip stage'):
                    self.tool['exception_tables']()
                skip.write_text('instruction count budgets\twhy\tlane\t2026-10-21\n')
                self.assertIn('full-only::case', self.tool['exception_tables']()[1])

    def test_missing_table_fails_before_execution(self):
        with tempfile.TemporaryDirectory() as temporary:
            with patch.dict(self.tool['run_stages'].__globals__, TREE=Path(temporary)), \
                    patch('subprocess.run', side_effect=AssertionError('must not run')):
                with self.assertRaisesRegex(self.tool['TableError'], r'main-runs-skip.tsv: No such file'):
                    self.tool['exception_tables']()

    def test_configuration_failure_is_reported_nonzero_and_does_not_cover_a_tick(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            with patch.dict(self.tool['run'].__globals__, RUNS=root, ROWS=root / 'runs.jsonl',
                            run_stages=unusable_table(self.tool), attribute=lambda *args: [],
                            rows=lambda: [], push_to_main=lambda *args: self.fail('must not publish')):
                self.assertEqual(self.tool['run']('abc', 'fast'), 1)
            history = json.loads((root / 'runs.jsonl').read_text())
            self.assertEqual(history['reds'], {})
            self.assertFalse(self.tool['covers'](history, 'fast'))
            self.assertIn('known.tsv:2: empty required field 2', (root / 'latest.md').read_text())
            self.assertIn('0 code reds, 0 infra reds, 1 configuration errors', (root / 'latest.md').read_text())


def unusable_table(tool):
    def run(*_):
        raise tool['TableError']('known.tsv:2: empty required field 2')
    return run


class Lifecycle(unittest.TestCase):
    def setUp(self):
        self.tool = runpy.run_path(str(ROOT / 'tools/main-runs'))
        self.events = self.tool['event_logs']

    def result(self, cases, status=None, execution='fresh', truncated=False):
        details = f'running {len(cases)} tests\n' + ''.join(f'test {name} ... {state}\n' for name, state in cases)
        if not truncated:
            details += ('test result: ok. ' + '; '.join(f'{sum(state == value for _, state in cases)} {label}'
                        for value, label in [('ok', 'passed'), ('FAILED', 'failed'), ('ignored', 'ignored')]) + '; 0 measured; 0 filtered out;\n')
        verdict = status or (self.events.FAIL if any(state == 'FAILED' for _, state in cases) else self.events.PASS)
        return dict(status=verdict, details=details,
                    provenance=dict(execution=execution, command_succeeded=verdict == self.events.PASS))

    def observe(self, results, known, command=None, inventory_complete=False, skipped=False, decoder_error=False, inventory=None):
        known = {name: [name, 'reason', 'lane', '2026-10-21'] for name in known}
        command = command or ['buck2', 'test', '//:binary']
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            logs = directory / 'logs/workspace-tests'
            logs.mkdir(parents=True)
            (logs / 'test.pb.zst').touch()
            def stages(stage, tier):
                stage('workspace tests', command, target_inventory=inventory if inventory is not None else (command[2:] if inventory_complete else None))
            with patch.dict(self.tool['run_stages'].__globals__, stages=stages, stage_plan=lambda tier: {},
                            exception_tables=lambda: ({'workspace tests': ['workspace tests', 'why', 'lane', '2026-10-21']} if skipped else {},
                                                       known, {'known-failures.tsv': dict(sha256='digest')})), \
                    patch('subprocess.run', return_value=subprocess.CompletedProcess([], 0)), \
                    patch.object(self.events, 'test_results', side_effect=ValueError('truncated') if decoder_error else None,
                                 return_value=results):
                with (directory / 'run.log').open('w') as log:
                    reds, ran, skip, perf, status = self.tool['run_stages'](directory, log, 'fast')
        return reds, status

    def test_killed_process_is_infrastructure_even_with_partial_success_and_a_known_row(self):
        for code, known in [(137, []), (137, ['binary']), (1, [])]:
            with self.subTest(code=code, known=known):
                result = self.result([('prefix', 'ok')], truncated=True, status=self.events.FAIL)
                result['details'] += 'test unfinished ... \n---- STDERR ----\nKilled\n'
                result['provenance'].update(signed_exit_code=code, command_succeeded=False)
                reds, status = self.observe([('binary', result)], known)
                self.assertEqual(reds['workspace tests: binary']['kind'], 'infra' if code == 137 else 'code')
                observation = next(o for o in status['observations'] if o['scope'] == 'target')
                self.assertEqual(observation['state'], 'infrastructure' if code == 137 else 'failure')
                self.assertFalse(observation['complete'])

    def test_timed_failure_uses_its_case_exception_without_suppressing_a_new_failure(self):
        result = self.result([('known_case', 'FAILED')])
        result['details'] = result['details'].replace('... FAILED\n', '... FAILED <0.123s>\n')
        command = ['buck2', 'test', '//:binary-shard-1']
        reds, _ = self.observe([('binary-shard-1', result)], ['binary::known_case'], command=command)
        self.assertEqual(reds, {})
        result['details'] = result['details'].replace('known_case', 'new_case')
        reds, _ = self.observe([('binary-shard-1', result)], ['binary::known_case'], command=command)
        self.assertEqual(reds['workspace tests: binary-shard-1']['ids'], ['binary::new_case'])

    def test_failed_child_stdout_belongs_to_its_started_parent(self):
        result = self.result([('parent', 'FAILED')])
        result['details'] = result['details'].replace('test parent', 'HIRSEL_TEST_STARTED parent\ntest parent')
        result['details'] += '\nrunning 1 test\ntest delegated_worker ... FAILED\n'
        reds, status = self.observe([('binary', result)], ['binary::parent'])
        self.assertEqual(reds, {})
        cases = [o['identity'] for o in status['observations'] if o['scope'] == 'case']
        self.assertEqual(cases, ['binary::parent'])

    def test_expired_cas_upload_is_infrastructure_with_no_executed_bodies(self):
        result = dict(status=self.events.FAIL,
                      details='---- STDOUT ----\n\n---- STDERR ----\n'
                              'Your build requires an artifact that has expired in the RE CAS and Buck does not have it.\n',
                      provenance=dict(execution='fresh', command_succeeded=False, signed_exit_code=None))
        reds, status = self.observe([('binary', result)], [])
        self.assertEqual(reds['workspace tests: binary']['kind'], 'infra')
        observation = next(o for o in status['observations'] if o['scope'] == 'target')
        self.assertEqual(observation['state'], 'infrastructure')
        self.assertFalse(observation['complete'])

    def test_stage_exception_runs_and_preserves_failure_recovery_and_infrastructure(self):
        identity = 'stage: cost coverage'
        known = {identity: [identity, 'missing measured rows', 'lane', '2026-10-22']}
        for exit_status, expected in [(1, 'failure'), (0, 'pass'), (71, 'infrastructure')]:
            with self.subTest(exit_status=exit_status), tempfile.TemporaryDirectory() as temporary:
                directory = Path(temporary)
                def stages(stage, tier):
                    stage('cost coverage', [sys.executable, '-c', f'raise SystemExit({exit_status})'])
                with patch.dict(self.tool['run_stages'].__globals__, stages=stages,
                                stage_plan=lambda tier: {}, exception_tables=lambda: ({}, known, {})):
                    with (directory / 'run.log').open('w') as log:
                        reds, ran, _, _, status = self.tool['run_stages'](directory, log, 'fast')
                self.assertEqual(ran, ['cost coverage'])
                self.assertEqual(status['known'][0]['states'], [expected])
                self.assertEqual(status['known'][0]['recovery_stages'], ['cost coverage'] if exit_status == 0 else [])
                self.assertEqual(status['known'][0]['observations'][0]['scope'], 'stage')
                if exit_status == 71:
                    self.assertEqual(reds[identity]['kind'], 'infra')
                else:
                    self.assertEqual(reds, {})

    def test_exact_fail_pass_and_unobserved_full_only_case(self):
        reds, status = self.observe([('binary', self.result([('broken', 'FAILED'), ('fixed', 'ok')]))],
                                    ['binary::broken', 'binary::fixed', 'full-only::case'])
        entries = {entry['identity']: entry for entry in status['known']}
        self.assertEqual(reds, {})
        self.assertEqual(entries['binary::broken']['states'], ['failure'])
        self.assertEqual(entries['binary::fixed']['recovery_stages'], ['workspace tests'])
        self.assertEqual(entries['full-only::case']['states'], ['unobserved'])
        self.assertEqual(entries['full-only::case']['recovery_stages'], [])
        self.assertEqual(entries['binary::fixed']['observations'][0]['provenance']['execution'], 'fresh')

    def test_target_needs_complete_fresh_population(self):
        good = self.result([('one', 'ok'), ('two', 'ok')])
        for results, command, complete, expected in [
                ([('binary', good)], None, False, True),
                ([('binary', self.result([('one', 'ok'), ('two', 'FAILED')]))], None, False, False),
                ([('binary', self.result([('one', 'ok')], truncated=True))], None, False, False),
                ([('binary', good)], ['buck2', 'test', '//:binary', '--test-arg=one'], False, False),
                ([('binary-shard-1', good)], ['buck2', 'test', '//:binary-shard-1'], False, False),
                ([('binary-shard-1', good)], ['buck2', 'test', '//:binary-shard-1', '//:binary-shard-2'], True, False),
                ([('binary-shard-1', good), ('binary-shard-2', good)],
                 ['buck2', 'test', '//:binary-shard-1', '//:binary-shard-2'], True, True),
                ([('binary', good), ('binary', good)], None, False, False)]:
            with self.subTest(command=command, results=results):
                _, status = self.observe(results, ['binary'], command, complete)
                self.assertEqual(bool(status['known'][0]['recovery_stages']), expected)
                self.assertEqual(len([o for o in status['observations'] if o['scope'] == 'target'
                                      and o['provenance'].get('event_log')]), len(results))

    def test_shard_one_of_many_and_filtered_launcher_output_cannot_recover_target(self):
        good = self.result([('one', 'ok')])
        _, status = self.observe([('binary-shard-1', good)], ['binary'],
                                 command=['buck2', 'test', '//:binary-shard-1'],
                                 inventory=['//:binary-shard-1', '//:binary-shard-2'])
        self.assertEqual(status['known'][0]['recovery_stages'], [])
        good['details'] = good['details'].replace('0 filtered out', '8 filtered out')
        _, status = self.observe([('binary', good)], ['binary', 'binary::one'])
        self.assertEqual(status['known'][0]['recovery_stages'], [])
        self.assertEqual(status['known'][1]['recovery_stages'], ['workspace tests'])

    def test_cache_skip_omission_infra_missing_and_truncation_are_not_recovery(self):
        for name, result in [
                ('cache', self.result([('case', 'ok')], execution='cached')),
                ('unknown', self.result([('case', 'ok')], execution='unknown')),
                ('skip', self.result([('case', 'ignored')], status=self.events.SKIP)),
                ('omitted', self.result([('case', 'ok')], status=self.events.OMITTED)),
                ('infra', self.result([('case', 'ok')], status=self.events.INFRA_FAILURE)),
                ('fatal', self.result([('case', 'ok')], status=self.events.FATAL)),
                ('truncated', self.result([('case', 'ok')], truncated=True))]:
            with self.subTest(name=name):
                reds, status = self.observe([('binary', result)], ['binary::case', 'binary'])
                for entry in status['known']:
                    self.assertEqual(entry['recovery_stages'], [])
                if name == 'infra':
                    self.assertEqual(reds['workspace tests: binary']['kind'], 'infra')
                    self.assertEqual(status['known'][0]['states'], ['infrastructure'])
        for kwargs in ({}, {'skipped': True}, {'decoder_error': True}):
            _, status = self.observe([], ['binary::case', 'binary'], **kwargs)
            self.assertTrue(all(not entry['recovery_stages'] for entry in status['known']))
            self.assertEqual(status['known'][0]['states'], ['unobserved'])

    def test_selected_missing_result_is_unusable_and_duplicate_case_does_not_recover(self):
        reds, status = self.observe([], ['binary'])
        self.assertEqual(status['known'][0]['states'], ['unusable'])
        self.assertEqual(reds['evidence: workspace tests: binary']['kind'], 'infra')
        good = self.result([('case', 'ok')])
        _, status = self.observe([('binary', good), ('binary', good)], ['binary::case'])
        self.assertEqual(status['known'][0]['recovery_stages'], [])
        good['provenance']['command_succeeded'] = False
        _, status = self.observe([('binary', good)], ['binary', 'binary::case'])
        self.assertTrue(all(not entry['recovery_stages'] for entry in status['known']))

    def test_parent_pass_never_invents_an_unobserved_child(self):
        _, status = self.observe([('binary', self.result([('other', 'ok')]))], ['binary::case'])
        self.assertEqual(status['known'][0]['states'], ['unobserved'])
        self.assertEqual(status['known'][0]['context'][0]['state'], 'pass')

    def test_lifecycle_report_and_history_keep_scope_and_cache(self):
        _, status = self.observe([('binary', self.result([('fixed', 'ok'), ('cached', 'ok')], execution='cached'))],
                                  ['binary::fixed', 'binary::absent'])
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            with patch.dict(self.tool['run'].__globals__, RUNS=root, ROWS=root / 'runs.jsonl',
                            run_stages=lambda *args: ({}, ['workspace tests'], [], None, status),
                            attribute=lambda *args: [], rows=lambda: [], push_to_main=lambda *args: None):
                self.tool['run']('abc', 'fast')
            history = json.loads((root / 'runs.jsonl').read_text())
            self.assertEqual(history['exception_status'], status)
            report = (root / 'latest.md').read_text()
            for text in ('SHA abc; tier fast', 'sha256 digest', 'binary::fixed', 'pass (cached)',
                         'recovery candidate stages: none', 'binary::absent: unobserved'):
                self.assertIn(text, report)


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


class Attribution(unittest.TestCase):
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
        self.tool = runpy.run_path(str(ROOT / 'tools/main-runs'))
        self.attribute = self.tool['attribute']
        self.attribute.__globals__.update(TREE=self.repo)

    def git(self, *arguments):
        return subprocess.run(['git', *arguments], cwd=self.repo, env=self.env, check=True, capture_output=True,
                              text=True).stdout.strip()

    def test_report_names_new_owners_and_keeps_existing_attribution(self):
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
        with patch.dict(self.attribute.__globals__, RUNS=self.repo, ROWS=self.repo / 'runs.jsonl',
                        run_stages=lambda *args: (reds, ['workspace tests'], [], None, empty_status()),
                        rows=lambda: history, push_to_main=lambda *args: None):
            self.tool['run'](self.commits['bar'], 'fast')
        report = (self.repo / 'latest.md').read_text()
        owners = report.split('## Owners\n\n', 1)[1].split('\n\n', 1)[0]
        self.assertEqual(owners,
                         '- conformance validation and ratchet: gate-conformance-shard-3 -> lane-b '
                         f'({self.commits["bar"]})\n'
                         '- stage: native and Lean checks -> unowned\n'
                         '- workspace tests: hirsel-foo-test-it-shard-2 -> lane-b '
                         f'({self.commits["bar"]})')
        foo, corpus, stage, old = reds.values()
        # A red in a stage the fast run ran is new since that run, so only bar can be its culprit;
        # the corpus red's stage last ran in the full run, so its culprit is among all three commits.
        self.assertEqual((foo['culprit'], foo['lane']), (self.commits['bar'], 'lane-b'))
        self.assertEqual((corpus['culprit'], corpus['lane']), (self.commits['bar'], 'lane-b'))
        self.assertEqual((stage['culprit'], stage['lane']), (None, None))
        self.assertEqual((old['culprit'], old['lane'], old['since']), ('c0ffee', 'lane-z', '5eed'))

    def test_exit_71_stage_has_no_owner(self):
        # Only lane-b landed since the preceding run: a generic stage failure
        # would be blamed on it. Exercise stage execution through attribution.
        namespace = self.attribute.__globals__
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            with patch.dict(namespace, stage_plan=lambda tier: {}, exception_tables=lambda: ({}, {}, {}), stages=lambda stage, tier: stage(
                    'memory guard', [namespace['PY'], '-c', 'raise SystemExit(71)'])):
                with (directory / 'run.log').open('w+') as log:
                    reds, *_ = namespace['run_stages'](directory, log, 'fast')
            self.assertEqual(self.attribute(self.commits['bar'], reds,
                             [dict(sha=self.commits['foo-2'], reds={})]), [])
        self.assertIsNone(reds['stage: memory guard']['culprit'])
        self.assertIsNone(reds['stage: memory guard']['lane'])

    def test_infra_has_no_owner_and_does_not_reuse_old_lane_blame(self):
        reds = {f'workspace tests: {name}': dict(stage='workspace tests', target=name, ids=[],
                                               kind='infra', reason=reason)
                for name, reason in (('oom', 'memory limit'), ('slow', 'timeout'))}
        history = [dict(sha=self.commits['foo-2'], reds={
            key: dict(culprit='c0ffee', lane='lane-a', since='5eed') for key in reds})]
        self.assertEqual(self.attribute(self.commits['bar'], reds, history), [])
        for red in reds.values():
            self.assertIsNone(red['culprit'])
            self.assertIsNone(red['lane'])
        # Infra stays out of lane-blame history; a code failure replacing it is new.
        self.assertEqual(self.attribute(self.commits['bar'], reds,
                         [dict(sha=self.commits['bar'], reds={})]), [])
        code = {'workspace tests: oom': dict(stage='workspace tests', target='oom', ids=[], kind='code')}
        self.assertEqual(self.attribute(self.commits['bar'], code,
                         [dict(sha=self.commits['foo-2'], reds={})]),
                         [f'- workspace tests: oom -> lane-b ({self.commits["bar"]})'])

    def test_configuration_error_has_no_lane_blame(self):
        reds = {'configuration: tables': dict(stage='configuration', target=None, ids=[],
                                             kind='configuration', reason='known.tsv:2: duplicate identity')}
        self.assertEqual(self.attribute(self.commits['bar'], reds,
                         [dict(sha=self.commits['foo-2'], reds={})]), [])
        self.assertIsNone(reds['configuration: tables']['culprit'])


class Tiers(unittest.TestCase):
    def test_tsan_wrapper_is_not_run_without_its_buck_inputs(self):
        tool = runpy.run_path(str(ROOT / 'tools/main-runs'))
        commands = {}
        with patch.dict(os.environ), patch('subprocess.run', return_value=subprocess.CompletedProcess([], 0, '', '')):
            tool['stages'](lambda name, command, **_: commands.update({name: command}), 'fast')
        self.assertFalse(any('io_real_tsan.py' in str(command) for command in commands.values()))
        for test in (ROOT / 'tools/tests').glob('*.py'):
            self.assertIn(f'tool tests: tools/tests/{test.name}', commands)

    def test_fast_build_excludes_campaigns_and_keeps_priority_40(self):
        tool = runpy.run_path(str(ROOT / 'tools/main-runs'))
        queries, stages = [], {}
        def query(command, **kwargs):
            queries.append(command[-1])
            return subprocess.CompletedProcess(command, 0, '//:workspace-lib\n', '')
        with patch.dict(os.environ), patch('subprocess.run', side_effect=query):
            tool['stages'](lambda name, command, **_: stages.update({name: command}), 'fast')
        command = stages['workspace build']
        self.assertIn('hirsel.priority=40', command)
        self.assertIn('//:workspace-lib', command)
        self.assertNotIn('//:', command)
        self.assertTrue(any('attrfilter(labels, gate' in q and '_hirsel_precheck' in q for q in queries))

    def test_workspace_test_inventory_excludes_build_only_fixtures(self):
        tool = runpy.run_path(str(ROOT / 'tools/main-runs'))
        queries = []
        def query(command, **kwargs):
            queries.append(command[-1])
            return subprocess.CompletedProcess(command, 0, '', '')
        with patch.dict(os.environ), patch('subprocess.run', side_effect=query):
            tool['stages'](lambda *args, **kwargs: None, 'full')
        test_queries = [q for q in queries if q.startswith(('(kind(', 'kind('))]
        self.assertEqual(len(test_queries), 2)
        for expression in test_queries:
            self.assertIn('- attrfilter(labels, fixture, //...)', expression)

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
                                'workspace build', 'workspace clippy', 'formatting', 'workspace tests', 'free-mode GenMC models',
                                'fresh std instruction budget'])
        self.assertEqual(named['full'][:len(named['fast'])], named['fast'])
        self.assertIn('native passing corpus with independent second hash seed', named['full'])
        # A fast tick is skipped by a full run of main; a full tick is not skipped by a fast run.
        covers = tool['covers']
        self.assertTrue(covers({}, 'fast') and covers({'tier': 'full'}, 'fast') and covers({}, 'full'))
        self.assertFalse(covers({'tier': 'fast'}, 'full'))


class BoundedTests(unittest.TestCase):
    def test_run_bounds_buck_requests_without_leaking_the_bound_into_tool_tests(self):
        tool = runpy.run_path(str(ROOT / 'tools/main-runs'))
        environments = []
        def stages(stage, tier):
            stage('workspace tests', ['buck2', 'test'])
            stage('tool tests: isolated', ['python3', 'tool.py'], isolated=True)
        def execute(command, **kwargs):
            environments.append(kwargs['env'])
            return subprocess.CompletedProcess(command, 0)
        with tempfile.TemporaryDirectory() as temporary, \
             patch.dict(tool['run_stages'].__globals__, stage_plan=lambda tier: {},
                        exception_tables=lambda: ({}, {}, {}), stages=stages), \
             patch('subprocess.run', side_effect=execute):
            directory = Path(temporary)
            with (directory / 'run.log').open('w+') as log:
                tool['run_stages'](directory, log, 'fast')
        self.assertEqual(environments[0]['HIRSEL_TEST_BATCH_SIZE'], '32')
        self.assertNotIn('HIRSEL_TEST_BATCH_SIZE', environments[1])
        self.assertEqual(environments[0]['HIRSEL_TEST_BATCH_CONCURRENCY'], '8')
        self.assertNotIn('HIRSEL_TEST_BATCH_CONCURRENCY', environments[1])


class ReportMemory(unittest.TestCase):
    def test_history_retains_attribution_without_accumulating_archival_status(self):
        tool = runpy.run_path(str(ROOT / 'tools/main-runs'))
        peaks = []
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / 'runs.jsonl'
            for count in (2, 4, 8):
                with path.open('w') as out:
                    for n in range(count):
                        json.dump(dict(sha=str(n), start=n, tier='fast', stages=['tests'],
                                       reds={'tests: binary': dict(culprit='abc', lane='owner', since='base')},
                                       exception_status=dict(observations=['x' * 200000])), out)
                        out.write('\n')
                with patch.dict(tool['rows'].__globals__, ROWS=path):
                    tracemalloc.start()
                    history = tool['rows']()
                    peaks.append(tracemalloc.get_traced_memory()[1])
                    tracemalloc.stop()
                self.assertEqual(len(history), count)
                self.assertEqual(history[-1]['reds']['tests: binary']['lane'], 'owner')
                self.assertTrue(all('exception_status' not in row for row in history))
            self.assertLess(peaks[-1], peaks[0] * 1.5)

    def test_publish_streams_json_and_preserves_newer_latest_and_history_order(self):
        tool = runpy.run_path(str(ROOT / 'tools/main-runs'))
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            status, histories = empty_status(), []
            def attribute(sha, reds, history):
                histories.append(history)
                return []
            with patch.dict(tool['publish'].__globals__, RUNS=root, ROWS=root / 'runs.jsonl',
                            attribute=attribute, push_to_main=lambda *args: self.fail('offline publication builds nothing')), \
                    patch('json.dumps', side_effect=AssertionError('must stream the row')):
                for start in (20, 10):
                    tool['publish'](str(start), 'fast', root / f'run-{start}', start,
                                    {}, ['tests'], [], None, status, update_main=False, finished=start + 60)
                self.assertIn('main run 20:', (root / 'latest.md').read_text())
                self.assertTrue((root / 'reports/run-10.md').exists())
                self.assertEqual([row['sha'] for row in tool['rows']()], ['10', '20'])
                self.assertEqual(histories, [[], []])
            with (root / 'runs.jsonl').open() as source:
                rows = [json.loads(line) for line in source]
            self.assertEqual(rows[-1]['exception_status'], status)
            self.assertEqual(rows[-1]['minutes'], 1)

    def test_decoder_streams_irrelevant_actions_and_joins_late_execution(self):
        tool = runpy.run_path(str(ROOT / 'tools/main-runs'))
        events = tool['event_logs']
        result = {'Event': {'data': {'Instant': {'data': {'TestResult': dict(
            name='root//:binary', status=events.FAIL, details='test broken ... FAILED', duration_us=1000)}}}}}
        run = {'Event': {'data': {'SpanEnd': {'data': {'TestRun': {
            'suite': {'target_label': {'label': {'name': 'binary'}}, 'suite_name': 'binary'},
            'command_report': {'status': {'Failure': {}}, 'details': {
                'signed_exit_code': 1, 'command_kind': {'command': {'LocalCommand': {}}}}},
        }}}}}}
        def lines(_):
            yield json.dumps(result)
            for _ in range(1000):
                yield json.dumps({'Event': {'irrelevant_action': 'x' * 4000}})
            yield json.dumps(run)
        with patch.object(events, 'lines', lines), \
                patch('subprocess.run', side_effect=AssertionError('must stream Buck output')):
            tracemalloc.start()
            decoded = events.test_results('fixture', history=events.NoTestHistory(), all_results=True)
            peak = tracemalloc.get_traced_memory()[1]
            tracemalloc.stop()
        self.assertLess(peak, 500000)
        self.assertEqual(tool['failing_ids'](decoded[0][0], decoded[0][1]['details']), ['binary::broken'])
        self.assertEqual(decoded[0][1]['provenance']['execution'], 'fresh')


class ExpiredArtifacts(unittest.TestCase):
    def test_retry_once_with_only_own_daemon_and_final_attempt_evidence(self):
        for final in (0, 1):
            with self.subTest(final=final), tempfile.TemporaryDirectory() as temporary:
                tool = runpy.run_path(str(ROOT / 'tools/main-runs'))
                directory, calls = Path(temporary), []
                command = ['fixture-stage']
                def execute(arguments, **kwargs):
                    calls.append((arguments, kwargs))
                    if len(calls) == 1 or final and len(calls) == 3:
                        kwargs['stdout'].write('Your build requires an artifact that has expired in the RE CAS\n')
                        kwargs['stdout'].flush()
                        return subprocess.CompletedProcess(arguments, 1)
                    return subprocess.CompletedProcess(arguments, 0)
                with patch.dict(tool['run_stages'].__globals__, stage_plan=lambda _: {},
                                exception_tables=lambda: ({}, {}, {}),
                                stages=lambda stage, _: stage('tests', command)), \
                        patch('subprocess.run', side_effect=execute):
                    with (directory / 'run.log').open('w') as log:
                        reds, _, _, _, status = tool['run_stages'](directory, log, 'fast')
                self.assertEqual([args for args, _ in calls], [command, [str(ROOT / '.buck2/bin/buck2'), 'kill'], command])
                self.assertEqual(calls[1][1]['cwd'], ROOT)
                self.assertEqual(calls[2][1]['env']['HIRSEL_BUCK2_LOG_DIR'], str(directory / 'logs/tests/retry'))
                self.assertEqual(len(status['selections'][0]['attempts']), 2)
                self.assertEqual(reds['stage: tests']['kind'] if final else reds, 'infra' if final else {})


class RollingBatches(unittest.TestCase):
    def test_replenishes_small_batches_without_exceeding_original_request_cap(self):
        tool = runpy.run_path(str(ROOT / 'tools/main-runs'))
        events = tool['event_logs']
        release, lock = threading.Event(), threading.Lock()
        active, peak, ran, overlapped = 0, 0, [], []
        targets = [f'//:t-{i}' for i in range(8)]
        def execute(arguments, **kwargs):
            nonlocal active, peak
            selected = [arg for arg in arguments if arg in targets]
            with lock:
                active += len(selected)
                peak = max(peak, active)
                ran.extend(selected)
            if targets[0] in selected:
                overlapped.append(release.wait(2))
            if targets[4] in selected:
                release.set()
            with lock:
                active -= len(selected)
            return subprocess.CompletedProcess(arguments, int(targets[6] in selected), ''.join(selected), '')
        with tempfile.TemporaryDirectory() as temporary, patch.dict(os.environ, HIRSEL_BUCK2_LOG_DIR=temporary):
            result = events.run(['test', *targets, '--', '--timeout', '1800'], runner=execute,
                                batch_size=4, batch_concurrency=2)
        self.assertEqual(overlapped, [True])
        self.assertCountEqual(ran, targets)
        self.assertLessEqual(peak, 4)
        self.assertEqual(result.returncode, 1)
        self.assertEqual(result.stdout, ''.join(targets))


class PerfChecks(unittest.TestCase):
    def test_ratchet_records_large_drops_after_push_in_report_and_history(self):
        tool = runpy.run_path(str(ROOT / 'tools/main-runs'))
        counts = runpy.run_path(str(ROOT / 'tools/perf-counts'))
        profile = lambda n: dict(Ir=n, functions={'work': n})
        baseline = dict(large=profile(10000), boundary=profile(10000), noise=profile(10000))
        measured = dict(large=profile(8400), boundary=profile(8500), noise=profile(10001))
        for pushed in (True, False):
            with self.subTest(pushed=pushed), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                (root / 'tools').mkdir()
                shutil.copy(ROOT / 'tools/perf-counts', root / 'tools/perf-counts')
                table = root / 'tools/perf-counts.tsv'
                table.write_text(counts['table'](baseline))
                def push(name, update):
                    if name.startswith('Ratchet'):
                        update(root)
                        return 'cafebabe' if pushed else None
                output = io.StringIO()
                with patch.dict(tool['run'].__globals__, RUNS=root, ROWS=root / 'runs.jsonl',
                                run_stages=lambda *args: ({}, ['instruction count budgets'], [], measured, empty_status()),
                                attribute=lambda *args: [], rows=lambda: [], push_to_main=push), \
                        contextlib.redirect_stdout(output):
                    tool['run']('0123456789abcdef', 'full')
                actual = counts['read_table'](table)
                self.assertEqual(actual, baseline | dict(large=measured['large'], boundary=measured['boundary']))
                history = json.loads((root / 'runs.jsonl').read_text())
                expected = [dict(count='large', old=10000, new=8400,
                                 measured_sha='0123456789abcdef', pushed='cafebabe')] if pushed else []
                self.assertEqual(history['perf_checks'], expected)
                self.assertEqual('PERF-CHECK large 10000 -> 8400' in output.getvalue(), pushed)
                self.assertEqual('PERF-CHECK large 10000 -> 8400' in (root / 'latest.md').read_text(), pushed)
                self.assertNotIn('PERF-CHECK boundary', output.getvalue())

    def test_retry_reports_only_changes_against_the_new_baseline(self):
        tool = runpy.run_path(str(ROOT / 'tools/main-runs'))
        counts = runpy.run_path(str(ROOT / 'tools/perf-counts'))
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / 'tools').mkdir()
            shutil.copy(ROOT / 'tools/perf-counts', root / 'tools/perf-counts')
            table = root / 'tools/perf-counts.tsv'
            profile = lambda n: dict(Ir=n, functions={'work': n})
            table.write_text(counts['table']({'a': profile(10000)}))
            checks = []
            update = tool['ratchet']({'a': profile(8000)}, checks)
            update(root)
            self.assertEqual(checks, [dict(count='a', old=10000, new=8000)])
            # Another run won the push race with a lower count.
            table.write_text(counts['table']({'a': profile(7000)}))
            update(root)
            self.assertEqual(checks, [])
            self.assertEqual(counts['read_table'](table)['a']['Ir'], 7000)


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
            with patch.dict(namespace, stages=stages, stage_plan=lambda tier: {}, exception_tables=lambda: ({}, {name: [name, 'why', 'lane', '2026-10-21']
                            for name in ('known', 'slow')}, {})), \
                    patch.object(events, 'test_results', return_value=list(results.items())):
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
                            run_stages=lambda *args: (reds, ['x'], [], None, empty_status()),
                            attribute=lambda *args: [], rows=lambda: [], push_to_main=lambda *args: None):
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
    def test_compiler_source_text_is_not_an_infrastructure_diagnostic(self):
        sys.path.insert(0, str(ROOT / 'tools/buck2'))
        from run_failures import infra_reason
        source = (', sources: SourceMap { files: [SourceFile { '
                  'text: "timeout 20; connection refused; memory limit 2G reached" }] }')
        failure = 'Query { error: Diagnostics([OWN-2]), phase: None' + source
        self.assertIsNone(infra_reason(101, failure))
        self.assertEqual(infra_reason(101, 'timed out after 30 s' + source), 'timeout')
        self.assertEqual(infra_reason(101, failure + '\nchild timed out after 30 s'), 'timeout')
        self.assertEqual(infra_reason(71, failure), 'memory limit (exit 71)')

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
                                (0, 'Infra Failure \x1b[32m0\x1b[39m'),
                                (1, 'AssertionError: expected CPU bound not found in source: '
                                    '.env("HIRSEL_MEMORY_LIMIT", "2G")')):
            with self.subTest(status=status, details=details):
                self.assertIsNone(infra_reason(status, details))


if __name__ == '__main__':
    unittest.main()
