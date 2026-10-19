#!/usr/bin/env python3
"""tools/compiler-bench: instruction regression verdicts, the scoreboard's
pairing, reading `hirsel --timings=json`, corpus determinism and edits, and the
budget tracking every interval and phase the compiler reports."""
import importlib.machinery
import importlib.util
import json
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
BENCH = ROOT / 'tools/compiler-bench'
sys.path.insert(0, str(BENCH))
import corpus  # noqa: E402


def script(name):
    loader = importlib.machinery.SourceFileLoader(name.replace('-', '_'), str(BENCH / name))
    module = importlib.util.module_from_spec(importlib.util.spec_from_loader(loader.name, loader))
    loader.exec_module(module)
    return module


compare = script('compare')
run = script('run')
BUDGET = json.loads((BENCH / 'budget.json').read_text())


def cell(cid, walls, status='ok', phases=None, language='hirsel', mode='check', program='p', jobs=1, intervals=None):
    runs = [{'status': status, 'wall_s': w, 'cpu_s': w, 'rss_mib': 100.0, 'instructions_u': w * 1000000} for w in walls]
    summary = {m: run.summary([r[m] for r in runs]) for m in ('instructions_u', 'wall_s', 'cpu_s', 'rss_mib')}
    for part, values in (('phases', phases), ('intervals', intervals)):
        if values:
            summary[part] = {name: run.summary(v) for name, v in values.items()}
    return {'id': cid, 'program': program, 'language': language, 'mode': mode, 'jobs': jobs,
            'status': status, 'runs': runs, 'summary': summary}


def result(*cells):
    return {'schema': 1, 'suite': 'compiler-bench', 'cells': list(cells), 'pairs': run.pairs(list(cells))}


class Verdicts(unittest.TestCase):
    def verdict(self, before, after, metric='instructions_u'):
        rows, _ = compare.compare(result(before), result(after), BUDGET)
        return {r['metric']: r for r in rows}[metric]

    def test_a_quiet_slowdown_past_the_threshold_regresses(self):
        row = self.verdict(cell('c', [2.0, 2.01, 1.99]), cell('c', [2.6, 2.61, 2.59]))
        self.assertTrue(row['regressed'])
        self.assertAlmostEqual(row['delta'], 0.3)

    def test_one_percent_threshold_is_counted_without_a_time_noise_bound(self):
        self.assertFalse(self.verdict(cell('c', [1.0] * 3), cell('c', [1.009] * 3))['regressed'])
        self.assertTrue(self.verdict(cell('c', [1.0] * 3), cell('c', [1.011] * 3))['regressed'])

    def test_times_rss_and_phases_do_not_change_the_verdict(self):
        before = cell('c', [5.0] * 3, phases={'verify': [2.0] * 3}, intervals={'units': [2.0] * 3})
        after = cell('c', [50.0] * 3, phases={'verify': [30.0] * 3}, intervals={'units': [30.0] * 3})
        after['summary']['instructions_u'] = before['summary']['instructions_u']
        rows, _ = compare.compare(result(before), result(after), BUDGET)
        self.assertEqual([r['metric'] for r in rows], ['instructions_u'])
        self.assertFalse(rows[0]['regressed'])

    def test_legacy_baseline_without_counts_is_unusable(self):
        with tempfile.TemporaryDirectory() as scratch:
            path = Path(scratch) / 'legacy.json'
            legacy = cell('c', [1.0])
            del legacy['summary']['instructions_u']
            path.write_text(json.dumps(result(legacy)))
            with self.assertRaisesRegex(ValueError, 'no instructions:u'):
                compare.load(path)

    def test_a_cell_that_stops_completing_regresses(self):
        rows, notes = compare.compare(result(cell('c', [1.0] * 3)), result(cell('c', [9.0], status='cap')), BUDGET)
        self.assertEqual([r['metric'] for r in rows if r['regressed']], ['status'])
        self.assertIn('c: status ok -> cap', notes)

    def test_exit_status_follows_the_verdict(self):
        with tempfile.TemporaryDirectory() as scratch:
            base, slow, same = (Path(scratch) / n for n in ('base.json', 'slow.json', 'same.json'))
            base.write_text(json.dumps(result(cell('c', [2.0] * 3))))
            slow.write_text(json.dumps(result(cell('c', [3.0] * 3))))
            same.write_text(json.dumps(result(cell('c', [2.02] * 3))))
            status = [subprocess.run([sys.executable, str(BENCH / 'compare'), str(base), str(other)],
                                     capture_output=True).returncode for other in (slow, same)]
            self.assertEqual(status, [1, 0])



class Measurement(unittest.TestCase):
    def test_perf_counts_the_compiler_and_children_inside_one_capped_call(self):
        snapshots = [{'cpu15': {'total': 100, 'idle': 0}}, {'cpu15': {'total': 200, 'idle': 80}}]
        def execute(command, **kwargs):
            self.assertEqual(command[:3], [str(run.CAPPED), '--timeout', '10'])
            self.assertEqual(command[-3:], ['--', 'hirsel', 'check'])
            self.assertEqual(command[command.index('taskset') + 1:command.index('taskset') + 3], ['-c', '31'])
            Path(command[command.index('-o') + 1]).write_text('1.0 0.8 0.1 1024')
            perf_path = command[command.index('perf'):]
            Path(perf_path[perf_path.index('-o') + 1]).write_text('\n'.join(
                f'123;;{e};100;100;;' for e in ('task-clock', 'cycles', 'instructions', 'cycles:u', 'instructions:u')))
            return type('Completed', (), {'returncode': 0, 'stderr': ''})()
        with patch.object(run.subprocess, 'run', side_effect=execute), \
                patch.object(run, 'siblings', return_value=[15]), \
                patch.object(run, 'cpu_snapshot', side_effect=snapshots):
            measured, _ = run.measured(['hirsel', 'check'], {}, 10)
        self.assertEqual(measured['instructions_u'], 123)
        self.assertEqual(measured['cpu_s'], 0.9)
        self.assertEqual(measured['rss_mib'], 1)
        self.assertEqual(measured['sibling_busy_share']['cpu15'], 0.2)


class Timings(unittest.TestCase):
    REPORT = {'schema': 1, 'command': 'build', 'wall_ns': 712000000, 'untimed_ns': 9000000,
              'intervals': [{'interval': 'image open', 'wall_ns': 101000000}, {'interval': 'S1', 'wall_ns': 0}],
              'phases': [{'phase': 'parse', 'work_ns': 3000000, 'cpu_ns': 2900000, 'calls': 128},
                         {'phase': 'codegen', 'work_ns': 0, 'cpu_ns': 0, 'calls': 0}],
              'comptime_verify': {'work_ns': 0, 'cpu_ns': 0, 'calls': 0}, 'wait_ns': 0,
              'counts': {'units': 168}}

    def test_the_last_stderr_line_is_the_report(self):
        stderr = 'warning: something\n' + json.dumps(self.REPORT) + '\n'
        found = run.timings(stderr)
        self.assertEqual(found['intervals'], {'image open': 0.101, 'S1': 0.0, '(untimed)': 0.009})
        self.assertEqual(found['phases'], {'parse': 0.003})
        self.assertEqual(found['counts'], {'units': 168})

    def test_output_without_a_report_has_no_timings(self):
        self.assertIsNone(run.timings('error: no such file\n'))
        self.assertIsNone(run.timings(json.dumps(dict(self.REPORT, schema=2))))


class Scoreboard(unittest.TestCase):
    def test_pairs_divide_hirsel_by_the_paired_rust_mode(self):
        cells = [cell('p/hirsel/clif', [6.0] * 3, mode='clif'),
                 cell('p/rust/debug/j1', [2.0] * 3, language='rust', mode='debug', jobs=1),
                 cell('p/rust/check/j1', [1.0] * 3, language='rust', mode='check', jobs=1)]
        [pair] = run.pairs(cells)
        self.assertEqual((pair['rust'], pair['ratio']), ('p/rust/debug/j1', 3.0))


class Corpus(unittest.TestCase):
    def tree(self, root):
        return {str(p.relative_to(root)): p.read_bytes() for p in sorted(root.rglob('*')) if p.is_file()}

    def test_generation_is_deterministic_and_pairs_share_modules(self):
        with tempfile.TemporaryDirectory() as one, tempfile.TemporaryDirectory() as two:
            first = corpus.corpus(Path(one), scale=(3000,))
            corpus.corpus(Path(two), scale=(3000,))
            self.assertEqual(self.tree(Path(one)), self.tree(Path(two)))
            names = {(p.name, p.language) for p in first}
            for p in first:
                if 'pairs' in p.groups:
                    self.assertIn((p.name, 'rust' if p.language == 'hirsel' else 'hirsel'), names, p.name)
            sources = json.loads((Path(one) / 'hirsel/syn3k/hirsel.json').read_text())['sources']
            hirsel = sorted(s['module'][0] for s in sources if s['module'])
            rust = sorted(p.stem for p in (Path(one) / 'rust/syn3k/src').glob('*.rs') if p.stem != 'main')
            self.assertEqual(hirsel, rust)

    def test_each_edit_changes_exactly_one_marked_text(self):
        with tempfile.TemporaryDirectory() as scratch:
            corpus.corpus(Path(scratch), scale=(10000,))
            for language, copy in (('hirsel', Path(scratch) / 'hirsel/syn10k'), ('rust', Path(scratch) / 'rust/syn10k')):
                path, base, variants = corpus.edits(copy, language)
                self.assertEqual(set(variants), set(run.EDITS))
                for text in variants.values():
                    self.assertNotEqual(text, base)
                    self.assertEqual(len(text.splitlines()), len(base.splitlines()))

    def test_every_listed_program_exists(self):
        for relative in corpus.CONFORMANCE:
            self.assertTrue((ROOT / 'conformance' / relative).is_file(), relative)
        self.assertTrue(corpus.tour_programs())
        for path in corpus.tour_programs():
            self.assertTrue(path.is_file(), path)


TIMINGS = ROOT / 'crates/hirsel-intern/src/timings.rs'


class Budget(unittest.TestCase):
    @unittest.skipUnless(TIMINGS.exists(), 'hirsel_intern::timings is not on this tree yet')
    def test_every_reported_interval_and_phase_is_tracked_and_documented(self):
        source = TIMINGS.read_text()
        reported = {kind: set(re.findall(kind + r'::\w+ => "([^"]+)"', source)) for kind in ('Interval', 'Phase')}
        self.assertEqual(set(BUDGET['intervals']['names']), reported['Interval'] | {'(untimed)'})
        self.assertEqual(set(BUDGET['phases']['names']), reported['Phase'])

    def test_the_readme_documents_every_tracked_name(self):
        readme = (BENCH / 'README.md').read_text()
        for part in ('intervals', 'phases'):
            for name in BUDGET[part]['names']:
                self.assertIn(f'| `{name}` |', readme, name)


if __name__ == '__main__':
    unittest.main()
