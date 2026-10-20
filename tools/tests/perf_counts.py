#!/usr/bin/env python3
"""Instruction-budget boundaries, attribution and static target selection."""
import contextlib
import io
import json
from pathlib import Path
import runpy
import sys
import tempfile
import unittest
from unittest.mock import patch, Mock

sys.dont_write_bytecode = True
ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / 'tools/buck2'))
import graph
import impact
import gating

COUNTS = runpy.run_path(str(ROOT / 'tools/perf-counts'))
MAIN_RUNS = runpy.run_path(str(ROOT / 'tools/main-runs'))


def row(count, **functions):
    return {'Ir': count, 'functions': functions}


class PerfCounts(unittest.TestCase):
    def test_regressions_fail_but_improvements_and_exact_tolerance_pass(self):
        compare = COUNTS['compare']
        for count in (1, 9899, 9900, 10000, 10100):
            self.assertEqual(compare({'case': row(10000)}, {'case': row(count)}), [])
        self.assertTrue(compare({'case': row(10000)}, {'case': row(10101)}))
        self.assertTrue(compare({'case': row(99)}, {'case': row(100)}))

    def test_ratchet_combines_lanes_without_raising_a_count_or_recording_noise(self):
        before = {'a': row(10000, work=10000), 'b': row(10000), 'noise': row(10000)}
        first = dict(before, a=row(8000, faster=8000), noise=row(9900))
        second = dict(before, b=row(9000, work=9000))
        for measurements in ([first, second], [second, first]):
            actual, changes = COUNTS['ratchet'](before, measurements)
            self.assertEqual(actual, dict(before, a=first['a'], b=second['b']))
            self.assertEqual(changes, [dict(count='a', old=10000, new=8000),
                                       dict(count='b', old=10000, new=9000)])
        newer = dict(before, a=row(7000))
        self.assertEqual(COUNTS['ratchet'](newer, [first])[0], newer)

    def test_ratchet_refuses_missing_cases_and_malformed_measurements(self):
        for measured in ([], {}, {'a': None}, {'a': row(0)}, {'a': row(True)}, {'a': row(10, bad=-1)}):
            with self.subTest(measured=measured), self.assertRaises(ValueError):
                COUNTS['ratchet']({'a': row(100)}, [measured])

    def test_only_a_passing_complete_main_run_result_supplies_a_measurement(self):
        measured = {'a': row(100)}
        marker = COUNTS['MARKER'] + json.dumps(measured)
        for status, details, expected in (('pass', marker, measured),
                                          ('fail', marker, None),
                                          ('pass', '', None),
                                          ('pass', marker + '\n' + marker, None)):
            self.assertEqual(MAIN_RUNS['perf_measurement'](status, details), expected)

    def test_improved_case_passes_with_note_and_complete_landing_measurement(self):
        main = COUNTS['main']
        output = io.StringIO()
        with patch.object(sys, 'argv', ['perf-counts']), \
                patch.dict(main.__globals__, native_cases=Mock(return_value=[('a', [], b'')]),
                           micro_cases=Mock(return_value=[]), measure=Mock(return_value=row(80)),
                           read_table=Mock(return_value={'a': row(100)})), \
                contextlib.redirect_stdout(output):
            self.assertEqual(main(), 0)
        self.assertIn('improvement a: 100 -> 80 Ir; pass', output.getvalue())
        self.assertEqual(MAIN_RUNS['perf_measurement']('pass', output.getvalue()), {'a': row(80)})

    def test_regression_fails_without_a_landing_measurement(self):
        main = COUNTS['main']
        output = io.StringIO()
        with patch.object(sys, 'argv', ['perf-counts']), \
                patch.dict(main.__globals__, native_cases=Mock(return_value=[('a', [], b'')]),
                           micro_cases=Mock(return_value=[]), measure=Mock(return_value=row(102)),
                           read_table=Mock(return_value={'a': row(100)})), \
                contextlib.redirect_stdout(output):
            self.assertEqual(main(), 1)
        self.assertIn('+2.00%', output.getvalue())
        self.assertNotIn(COUNTS['MARKER'], output.getvalue())

    def test_missing_extra_and_removed_cases_fail_closed(self):
        compare = COUNTS['compare']
        self.assertIn('missing budget', compare({}, {'new': row(1)})[0])
        self.assertIn('did not run', compare({'removed': row(1)}, {})[0])

    def test_regression_prints_function_diff_including_new_hot_functions(self):
        before = {'bench': row(100, old=100)}
        after = {'bench': row(200, old=100, new=100)}
        text = '\n'.join(COUNTS['compare'](before, after))
        self.assertIn('+100.00%', text)
        self.assertIn('baseline cg_annotate top functions', text)
        self.assertIn('current cg_annotate top functions', text)
        self.assertIn('100 new', text)

    def test_reads_ir_event_and_refuses_incomplete_or_empty_profiles(self):
        count = COUNTS['instruction_count']
        self.assertEqual(count('events: Dr Ir Dw\nsummary: 99 123 88\n'), 123)
        for text in ('events: Ir\n', 'summary: 10\n', 'events: Ir\nsummary: 0\n'):
            with self.assertRaises(ValueError):
                count(text)

    def test_annotate_uses_function_totals_once_and_preserves_rust_names(self):
        self.assertEqual(COUNTS['annotate']('''
1,219 PROGRAM TOTALS
< 1,000 /worker/path/a.rs:
  1,000   hirsel::work
> 1,200 hirsel::work:
  1,000   /worker/path/a.rs
    200   /different/a.rs
>    19 new_function:???
'''), {'hirsel::work': 1200, 'new_function': 19})
        with self.assertRaises(ValueError):
            COUNTS['annotate']('malformed')

    def test_table_roundtrip_and_duplicate_rejection(self):
        rows = {'z': row(120, **{'hirsel::work': 120}), 'a': row(30, a=30)}
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'perf-counts.tsv'
            path.write_text(COUNTS['table'](rows))
            self.assertEqual(COUNTS['read_table'](path), rows)
            with path.open('a') as out:
                out.write('a\t30\t<total>\n')
            with self.assertRaises(ValueError):
                COUNTS['read_table'](path)

    def test_default_records_program_budgets_and_open_compiler_case_is_diagnostic(self):
        main = COUNTS['main']
        fixture = ('native/control', ['/bin/fixture'], b'checksum')
        for arguments, expected in [(['--record'], {'native/control'}),
                                    (['--record', '--filter', 'check/hello-web'], {'check/hello-web'})]:
            with self.subTest(arguments=arguments):
                output = io.StringIO()
                with patch.object(sys, 'argv', ['perf-counts', *arguments]), \
                        patch.dict(main.__globals__, native_cases=Mock(return_value=[fixture]),
                                   micro_cases=Mock(return_value=[]), measure=Mock(return_value=row(100, work=100))), \
                        contextlib.redirect_stdout(output):
                    self.assertEqual(main(), 0)
                records = [line.removeprefix(COUNTS['MARKER']) for line in output.getvalue().splitlines()
                           if line.startswith(COUNTS['MARKER'])]
                self.assertEqual(set(json.loads(records[0])), expected)
                if '--filter' not in arguments:
                    self.assertIn('check/hello-web: open:', output.getvalue())
                    self.assertIn('check/todo-api: open:', output.getvalue())

    def test_compiler_cases_are_explicitly_open_in_the_reviewed_table(self):
        text = COUNTS['table']({'micro/sha': row(100, work=100)})
        self.assertIn('# check/hello-web: open:', text)
        self.assertIn('# check/todo-api: open:', text)

    def test_native_entry_uses_runtime_fixture_and_original_checksum_oracle(self):
        cases = COUNTS['native_cases']
        with tempfile.TemporaryDirectory() as directory:
            build = Mock(return_value=Path('/bin/fixture'))
            with patch.dict(cases.__globals__, build_program=build):
                results = list(cases('/bin/hirsel', Path(directory), {}, 'native/binarytrees'))
            self.assertEqual(len(results), 1)
            name, command, expected = results[0]
            self.assertEqual(name, 'native/binarytrees-small')
            grants = json.loads(command[-1])
            self.assertEqual(grants['io']['dirs'][0]['path'], 'fixtures')
            self.assertRegex(expected, rb'^[0-9a-f]{16}\n$')
            generated = build.call_args.args[1]
            self.assertIn('file.read_bytes(directory, path.of("binarytrees-small.input"))', generated)
            self.assertIn('workload(input)', generated)
            self.assertEqual(generated.count('pub fn main('), 1)
            self.assertNotIn('path.of(".")', generated)

    def test_micro_contract_builds_each_program_with_small_args_and_checks_its_output(self):
        cases = COUNTS['micro_cases']
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            programs = root / 'bench/micro/programs'
            programs.mkdir(parents=True)
            for name in ('print', 'channel', 'sha'):
                (programs / (name + '.hir')).write_text('fixture')
            with patch.dict(cases.__globals__, ROOT=root), \
                    patch.dict(cases.__globals__, build_program=Mock(return_value=Path('/bin/fixture')),
                               run=Mock(return_value=b'checksum\n')):
                results = list(cases('/bin/hirsel', root, {}))
            self.assertEqual([name for name, _, _ in results], ['micro/channel', 'micro/print', 'micro/sha'])
            for name, command, expected in results:
                self.assertEqual(command[-1], '1' if name == 'micro/sha' else '1000')
                self.assertEqual(command[1:3], ['--hirsel-grants', root / 'bench/micro/grants.json'])
                self.assertEqual(expected, COUNTS['MICRO_OUTPUTS'][name.removeprefix('micro/')])
            with patch.dict(cases.__globals__, ROOT=root), \
                    patch.dict(cases.__globals__, build_program=Mock(), run=Mock()):
                self.assertEqual(list(cases('/bin/hirsel', root, {}, 'micro/missing')), [])

    def test_native_and_std_changes_select_counts_but_docs_do_not(self):
        for path in ('crates/hirsel-llvm/src/lib.rs', 'crates/hirsel-rt/src/alloc.rs',
                     'crates/hirsel-rc/src/lib.rs', 'crates/hirsel-lir/src/lib.rs',
                     'stdlib/std@1/list.hir', 'bench/native/programs/wordfreq.hir',
                     'bench/micro/manifest.json'):
            self.assertTrue(impact.perf_counts([path]), path)
        self.assertFalse(impact.perf_counts(['docs/building.md', 'tools/gate']))

    def test_selection_suggests_target_for_native_source(self):
        import gating
        # Exercise the actual Selection path, without a daemon or a real diff.
        selection = object.__new__(gating.Selection)
        selection.paths = ['crates/hirsel-llvm/src/lib.rs']
        selection.root = ROOT
        selection.classified = impact.classify(selection.paths, {'hirsel-llvm'}, (ROOT / 'BUCK').read_text())
        selection.graph = graph.Graph((ROOT / 'BUCK').read_text())
        with patch.object(selection, 'ratchet', return_value=set()), \
                patch.object(selection, 'std_dependents', return_value=[]):
            self.assertIn(impact.PERF_COUNTS, selection.suggest().tests)

    def test_per_push_script_target_is_selectable_with_its_inputs_and_dependencies(self):
        targets = graph.Graph((ROOT / 'BUCK').read_text())
        self.assertIn('gate-perf-counts', targets.all_tests())
        self.assertIn('gate-perf-counts', targets.owners(['tools/perf-counts.tsv']))
        self.assertIn('gate-perf-counts', targets.rdeps({'hirsel-cli-bin-hirsel'}))
        self.assertNotIn('gate-native-runtime-c', targets.all_tests())


class BlessCounts(unittest.TestCase):
    def test_pool_record_rewrites_table_and_failed_or_missing_records_leave_it_alone(self):
        gate = runpy.run_path(str(ROOT / 'tools/gate'))
        bless = gate['bless_perf_counts']
        import gating
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / 'tools').mkdir()
            (root / 'tools/perf-counts').write_text((ROOT / 'tools/perf-counts').read_text())
            baseline = root / 'tools/perf-counts.tsv'
            rows = {'native/example': row(100, work=100)}
            marker = COUNTS['MARKER'] + json.dumps(rows)
            original_cwd = Path.cwd()
            try:
                for status, details, success in [(gating.PASS, marker, True),
                                                  (gating.FAIL, marker, False),
                                                  (gating.PASS, '', False),
                                                  (gating.PASS, marker + '\n' + marker, False)]:
                    with self.subTest(status=status, details=details):
                        baseline.write_text('original')
                        with patch.dict(bless.__globals__, ROOT=root), \
                                patch.object(gating, 'run', return_value=Mock(returncode=0)), \
                                patch.object(gating, 'buck2', return_value=Mock(returncode=0)) as buck2, \
                                patch.object(gating, 'test_results', return_value={
                                    'gate-perf-counts': {'status': status, 'details': details}}), \
                                patch('subprocess.run'), contextlib.redirect_stdout(io.StringIO()):
                            self.assertEqual(bless(), 0 if success else 1)
                            self.assertIn('--test-arg=--record', buck2.call_args.args)
                        self.assertEqual(baseline.read_text(), COUNTS['table'](rows) if success else 'original')
            finally:
                import os
                os.chdir(original_cwd)


if __name__ == '__main__':
    unittest.main()
