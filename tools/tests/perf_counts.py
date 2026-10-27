#!/usr/bin/env python3
"""Instruction-budget boundaries, attribution and blessing."""
import contextlib
import io
import json
import os
from pathlib import Path
import runpy
import sys
import subprocess
import tempfile
import unittest
from unittest.mock import patch, Mock

sys.dont_write_bytecode = True
ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / 'tools/buck2'))
import event_logs

COUNTS = runpy.run_path(str(ROOT / 'tools/perf-counts'))


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

    def test_reads_ir_event_and_refuses_incomplete_or_empty_profiles(self):
        count = COUNTS['instruction_count']
        self.assertEqual(count('events: Dr Ir Dw\nsummary: 99 123 88\n'), 123)
        for text in ('events: Ir\n', 'summary: 10\n', 'events: Ir\nsummary: 0\n'):
            with self.assertRaises(ValueError):
                count(text)

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

class BigintMicroOracle(unittest.TestCase):
    def test_exact_independent_oracle_and_invocation_binding(self):
        reference = runpy.run_path(str(ROOT / 'tools/tests/bigint_oracle_10.py'))
        expected = b'4901293426875656068\n'
        self.assertEqual(f"{reference['checksum'](1000, 1, 0)}\n".encode(), expected)
        cases = COUNTS['micro_cases']
        build = Mock(return_value=Path('/scratch/bigint'))
        with patch.dict(cases.__globals__, build_program=build):
            actual = list(cases('hirsel', Path('/scratch'), {}, 'micro/bigint_mul'))
        self.assertEqual(actual, [('micro/bigint_mul', [Path('/scratch/bigint'),
                         '--hirsel-grants', ROOT / 'bench/micro/grants.json', '1000', '1', '0'], expected)])
        build.assert_called_once_with('hirsel', (ROOT / 'bench/micro/programs/bigint_mul.hir').read_text(),
                                      'micro-bigint_mul', Path('/scratch'), {})

    def test_missing_oracle_fails_before_build_or_execution(self):
        cases = COUNTS['micro_cases']
        outputs = dict(COUNTS['MICRO_OUTPUTS'])
        del outputs['bigint_mul']
        build = Mock()
        with patch.dict(cases.__globals__, MICRO_OUTPUTS=outputs, build_program=build):
            with self.assertRaisesRegex(ValueError, 'independent small-input output oracle'):
                list(cases('hirsel', Path('/scratch'), {}, 'micro/bigint_mul'))
        build.assert_not_called()

    def test_wrong_or_empty_stdout_cannot_produce_a_measurement(self):
        measure = COUNTS['measure']
        env = dict(HIRSEL_VALGRIND='/valgrind/bin/valgrind',
                   HIRSEL_VALGRIND_RUNTIME='/runtime', LLVM_SYS_231_PREFIX='/llvm')
        for stdout in (b'4901293426875656069\n', b'', b'4901293426875656068'):
            with self.subTest(stdout=stdout), \
                    patch.dict(measure.__globals__, run=Mock(return_value=stdout), annotate=Mock()) as scope:
                with self.assertRaisesRegex(ValueError, 'wrong benchmark checksum'):
                    measure('micro/bigint_mul', ['/binary', '1000', '1', '0'],
                            Path('/scratch'), env, COUNTS['MICRO_OUTPUTS']['bigint_mul'])
                scope['annotate'].assert_not_called()


class BlessCounts(unittest.TestCase):
    def test_pool_record_rewrites_table_and_failed_or_missing_records_leave_it_alone(self):
        bless = COUNTS['bless']
        rows = {'native/example': row(100, work=100)}
        marker = COUNTS['MARKER'] + json.dumps(rows)
        with tempfile.TemporaryDirectory() as directory:
            baseline = Path(directory) / 'perf-counts.tsv'
            for status, details, success in [(event_logs.PASS, marker, True),
                                              (event_logs.FAIL, marker, False),
                                              (event_logs.PASS, '', False),
                                              (event_logs.PASS, marker + '\n' + marker, False)]:
                with self.subTest(status=status, details=details):
                    baseline.write_text('original')
                    with patch.dict(bless.__globals__, BASELINE=baseline), \
                            patch('subprocess.run') as run, patch('pathlib.Path.exists', return_value=True), \
                            patch.object(event_logs, 'test_results', return_value={
                                'gate-perf-counts': {'status': status, 'details': details}}), \
                            contextlib.redirect_stdout(io.StringIO()):
                        self.assertEqual(bless(), 0 if success else 1)
                    self.assertIn('--test-arg=--record', run.call_args.args[0])
                    self.assertEqual(baseline.read_text(), COUNTS['table'](rows) if success else 'original')


class FreshStdCounts(unittest.TestCase):
    def test_whole_process_counters_must_be_positive_complete_and_unscaled(self):
        parse = COUNTS['perf_instructions']
        text = '# perf stat\n12345;;instructions:u;99;100.00;;\n12500;;instructions;99;100.00;;\n'
        self.assertEqual(parse(text), {'instructions:u': 12345, 'instructions': 12500})
        for broken in ('', text.splitlines()[1], text.replace('100.00', '99.99'),
                       text.replace('12345', '<not supported>'), text.replace('12345', '0'),
                       text + text, text.replace('100.00;;', '')):
            with self.subTest(text=broken), self.assertRaises(ValueError):
                parse(broken)

    def test_manual_budget_boundary_and_fresh_environment(self):
        fresh = COUNTS['fresh_std']
        with tempfile.TemporaryDirectory() as directory:
            baseline = Path(directory) / 'budget.json'
            original = json.dumps(dict(counter='instructions:u', jobs=1, instructions=10000,
                                       reason='Independent control baseline'))
            baseline.write_text(original)
            for count, expected in ((9900, 0), (10300, 0), (10301, 1)):
                def measure(command, **kwargs):
                    self.assertEqual(command[command.index('--cpu=60:60') + 1], '--')
                    self.assertIn('instructions:u,instructions', command)
                    self.assertEqual(command[-2:], ['check', str(ROOT / 'stdlib/std@1/hirsel.json')])
                    env = kwargs['env']
                    for key in ('HIRSEL_VERIFY_CACHE', 'HIRSEL_RESULTS_DIR', 'LD_PRELOAD'):
                        self.assertNotIn(key, env)
                    self.assertEqual((env['HIRSEL_JOBS'], env['HIRSEL_RESULTS'], env['HIRSEL_DAEMON']),
                                     ('1', 'none@1', 'off'))
                    self.assertFalse(Path(env['HIRSEL_PRODUCTS']).exists())
                    Path(command[command.index('-o') + 1]).write_text(
                        f'{count};;instructions:u;99;100.00;;\n{count + 100};;instructions;99;100.00;;\n')
                    return Mock(returncode=0)
                with patch.dict(fresh.__globals__, FRESH_BASELINE=baseline), \
                        patch.dict(os.environ, HIRSEL_VERIFY_CACHE='trust', LD_PRELOAD='/poison.so'), \
                        patch('subprocess.run', side_effect=measure), contextlib.redirect_stdout(io.StringIO()):
                    self.assertEqual(fresh(Path(__file__)), expected)
                self.assertEqual(baseline.read_text(), original)

    def test_failed_compiler_never_produces_a_passing_count(self):
        fresh = COUNTS['fresh_std']
        with tempfile.TemporaryDirectory() as directory:
            baseline = Path(directory) / 'budget.json'
            baseline.write_text(json.dumps(dict(counter='instructions:u', jobs=1, instructions=10000, reason='Control')))
            with patch.dict(fresh.__globals__, FRESH_BASELINE=baseline), \
                    patch('subprocess.run', side_effect=subprocess.CalledProcessError(137, ['compiler'])), \
                    self.assertRaises(subprocess.CalledProcessError):
                fresh(Path(__file__))


if __name__ == '__main__':
    unittest.main()
