#!/usr/bin/env python3
"""Retention protects in-flight logs and decodes archived commands without builds."""
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from concurrent.futures import ThreadPoolExecutor
from unittest.mock import patch

sys.dont_write_bytecode = True
sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'buck2'))
# This test has the module's filename, so load its implementation explicitly.
SPEC = importlib.util.spec_from_file_location('retained_events', Path(__file__).resolve().parents[1] / 'buck2/event_logs.py')
logs = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(logs)


class Retention(unittest.TestCase):
    def test_history_survives_rotation_and_concurrent_appends_have_one_header(self):
        with tempfile.TemporaryDirectory() as scratch:
            root = Path(scratch)
            sink = logs.TsvTestHistory(root / 'history/results.tsv')
            def append(index):
                sink.append([dict(gate=str(index), target='driver', shard=1,
                                  duration_seconds=2.5, date='2026-10-19', status=1)])
            with ThreadPoolExecutor(max_workers=8) as workers:
                list(workers.map(append, range(40)))
            logs.prune(root / 'logs', max_age=0, max_bytes=0)
            rows = sink.path.read_text().splitlines()
            self.assertEqual(len(rows), 41)
            self.assertEqual(rows[0], '\t'.join(sink.FIELDS))
            self.assertEqual({row.split('\t')[0] for row in rows[1:]}, {str(i) for i in range(40)})
            self.assertEqual(len(list(logs.test_history_rows(sink.path))), 40)

    def test_history_sink_is_host_configurable_and_can_be_disabled(self):
        with tempfile.TemporaryDirectory() as scratch:
            path = Path(scratch) / 'timings.tsv'
            with patch.dict(os.environ, {'HIRSEL_TEST_HISTORY': str(path)}):
                self.assertEqual(logs.test_history().path, path)
            with patch.dict(os.environ, {'HIRSEL_TEST_HISTORY': 'none'}):
                logs.test_history().append([{'not': 'a TSV row'}])
            self.assertFalse(path.exists())

    def test_age_and_size_prune_oldest_but_protect_live_writers_and_readers(self):
        with tempfile.TemporaryDirectory() as scratch:
            root = Path(scratch)
            old, newer = root / 'old.pb.zst', root / 'new.pb.zst'
            old.write_bytes(b'123456')
            newer.write_bytes(b'1234')
            os.utime(old, (100, 100))
            os.utime(newer, (200, 200))
            logs.prune(root, now=201, max_age=1000, max_bytes=5)
            self.assertFalse(old.exists())
            self.assertTrue(newer.exists())
            with logs.recorded('gate', root) as live:
                live.write_bytes(b'active')
                logs.prune(root, now=10**12, max_age=0, max_bytes=0)
                self.assertTrue(live.exists())
                self.assertEqual(logs.discover(root, 24), [])
            self.assertEqual(logs.discover(root, 24), [live])
            with logs.reading(live) as ready:
                self.assertTrue(ready)
                logs.prune(root, max_bytes=0)
                self.assertTrue(live.exists())
            logs.prune(root, max_bytes=0)
            self.assertFalse(live.exists())

    def test_failed_runs_are_retained_and_build_flags_do_not_change(self):
        with tempfile.TemporaryDirectory() as scratch:
            root = Path(scratch)
            buck = root / 'buck'
            buck.write_text('#!' + sys.executable + '\nimport sys,json,pathlib\n'
                            'args=sys.argv[1:]\n'
                            'if "--event-log" in args: pathlib.Path(args[args.index("--event-log")+1]).write_text("failure")\n'
                            'print(json.dumps(args))\nsys.exit(7 if args[0]=="build" else 0)\n')
            buck.chmod(0o755)
            with patch.dict(os.environ, {'HIRSEL_BUCK2_LOG_DIR': str(root / 'logs')}):
                result = logs.run(['build', '-c', 'hirsel.tests=focus', '//:one'], buck2=buck,
                                  capture_output=True, text=True)
                self.assertEqual(result.returncode, 7)
                args = json.loads(result.stdout)
                self.assertEqual([args[0], *args[3:]], ['build', '-c', 'hirsel.tests=focus', '//:one'])
                self.assertEqual(len(logs.discover(root / 'logs', 24)), 1)
                with self.assertRaises(RuntimeError), logs.recorded('interrupted') as failed:
                    failed.write_bytes(b'partial')
                    raise RuntimeError('interrupted')
                self.assertTrue(failed.exists())
                query = logs.run(['uquery', '//:one'], buck2=buck, capture_output=True, text=True)
                self.assertEqual(json.loads(query.stdout), ['uquery', '//:one'])

    def test_compressed_inputs_use_the_decoder_without_an_isolation_override(self):
        with tempfile.TemporaryDirectory() as scratch:
            root = Path(scratch)
            path = root / 'events.pb.zst'
            path.write_text('{"event": 1}\n')
            buck = root / 'buck'
            buck.write_text('#!' + sys.executable + '\nimport sys,pathlib\n'
                            'assert sys.argv[1:3]==["log","show"],sys.argv\n'
                            'print(pathlib.Path(sys.argv[3]).read_text(),end="")\n')
            buck.chmod(0o755)
            self.assertEqual(list(logs.lines(path, buck2=buck)), ['{"event": 1}\n'])


class BoundedTests(unittest.TestCase):
    def test_large_population_retains_every_target_option_log_and_failure(self):
        targets = [f'root//:cli-shard-{i}' for i in range(566)]
        prefix = ['test', '-c', 'hirsel.test_seed=123', '--target-platforms', '//tools/buck2:release']
        suffix = ['--', '--timeout', '1800']
        calls = []
        def execute(command, **kwargs):
            calls.append(command)
            Path(command[command.index('--event-log') + 1]).write_text('event')
            return subprocess.CompletedProcess(command, int(len(calls) == 1), str(len(calls)) + '\n', '')
        with tempfile.TemporaryDirectory() as temporary, \
             patch.dict(os.environ, HIRSEL_TEST_BATCH_SIZE='32', HIRSEL_BUCK2_LOG_DIR=temporary):
            result = logs.run(prefix + targets + suffix, runner=execute)
            self.assertEqual(len(logs.discover(temporary, 24)), 18)
        selected = [[arg for arg in call if arg.startswith('root//:')] for call in calls]
        self.assertEqual(len(calls), 18)
        self.assertTrue(all(0 < len(batch) <= 32 for batch in selected))
        self.assertEqual([target for batch in selected for target in batch], targets)
        self.assertEqual(len({call[3] for call in calls}), 18)
        for call in calls:
            self.assertEqual([call[1], *call[4:8]], prefix)
            self.assertEqual(call[-3:], suffix)
        self.assertEqual(result.returncode, 1)
        self.assertEqual(result.stdout, ''.join(f'{i}\n' for i in range(1, 19)))

    def test_non_test_and_small_commands_are_unchanged(self):
        for command in (['test', '//:one', '--', '--test-arg=full_gate_', '--test-arg=--ignored'],
                        ['build', *[f'//:lib-{i}' for i in range(65)]], ['uquery', '//:']):
            self.assertEqual(list(logs.test_batches(command, 32)), [command])


class Results(unittest.TestCase):
    def decode(self, sources, statuses=(logs.PASS,), all_results=False, exit_code=None):
        events = []
        for command in sources:
            events.append({'Event': {'data': {'SpanEnd': {'data': {'TestRun': {
                'suite': {'suite_name': 'binary', 'target_label': {
                    'label': {'package': 'root//', 'name': 'binary'}, 'configuration': {'full_name': 'dev#hash'}}},
                'command_report': {'status': {'Success': {}}, 'details': {
                    'command_kind': {'command': command}, 'metadata': {'start_time': [100, 0]},
                    **({'signed_exit_code': exit_code} if exit_code is not None else {})}},
            }}}}}})
        for status in statuses:
            events.append({'Event': {'timestamp': [101, 0], 'data': {'Instant': {'data': {'TestResult': {
                'name': 'root//:binary', 'status': status, 'details': 'test case ... ok', 'duration_us': 1000,
            }}}}}})
        with patch.object(logs, 'lines', return_value=iter(json.dumps(event) for event in events)):
            return logs.test_results('synthetic.pb.zst', history=logs.NoTestHistory(), all_results=all_results)

    def test_fresh_cache_local_absent_and_ambiguous_provenance(self):
        for sources, expected in [
                ([{'RemoteCommand': {'cache_hit': False, 'action_digest': 'digest'}}], 'fresh'),
                ([{'RemoteCommand': {'cache_hit': True, 'action_digest': 'digest'}}], 'cached'),
                ([{'LocalCommand': {}}], 'fresh'),
                ([{'RemoteCommand': {}}], 'unknown'),
                ([], 'unknown'),
                ([{'LocalCommand': {}}, {'RemoteCommand': {'cache_hit': True}}], 'unknown')]:
            with self.subTest(sources=sources):
                result = self.decode(sources)['binary']
                self.assertEqual(result['provenance']['execution'], expected)
                self.assertEqual(result['status'], logs.PASS)
                self.assertEqual(result['provenance']['event_timestamp'], [101, 0])
        result = self.decode([{'RemoteCommand': {'cache_hit': False, 'action_digest': 'digest'}}])['binary']
        self.assertEqual(result['provenance']['digest'], 'digest')
        self.assertEqual(result['provenance']['configuration'], {'full_name': 'dev#hash'})
        self.assertTrue(result['provenance']['command_succeeded'])

    def test_failed_process_preserves_its_signed_exit_code(self):
        result = self.decode([{'RemoteCommand': {'cache_hit': False}}],
                             statuses=(logs.FAIL,), exit_code=137)['binary']
        self.assertEqual(result['provenance']['signed_exit_code'], 137)
        self.assertFalse(result['provenance']['command_succeeded'])

    def test_all_observations_keeps_repeated_results_and_missing_results(self):
        observed = self.decode([{'LocalCommand': {}}], [logs.FAIL, logs.PASS], all_results=True)
        self.assertEqual([result['status'] for _, result in observed], [logs.FAIL, logs.PASS])
        self.assertEqual(self.decode([{'LocalCommand': {}}], [logs.FAIL, logs.PASS])['binary']['status'], logs.PASS)
        missing = self.decode([{'LocalCommand': {}}], [], all_results=True)
        self.assertTrue(missing[0][1]['missing_result'])
        self.assertEqual(missing[0][1]['provenance']['execution'], 'unknown')


if __name__ == '__main__':
    unittest.main()
