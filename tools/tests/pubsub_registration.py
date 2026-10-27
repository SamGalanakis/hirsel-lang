#!/usr/bin/env python3
"""Bijection and failure propagation for the original Pubsub recording cells."""
import contextlib
import importlib.util
import io
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
SUPPORT = ROOT / 'crates/hirsel-cli/tests/support/pubsub_relay_30'
sys.path.insert(0, str(SUPPORT))
import selection
import verify

CASES = json.loads((SUPPORT / 'cases.json').read_text())


class Registration(unittest.TestCase):
    def test_old_new_bijection_and_four_adapters(self):
        # Independent original cases x backend x width inventory, plus real hosts.
        old = {f'{case["id"]}-{backend}-{width}' for case in CASES
               for backend in ('llvm', 'cranelift') for width in (1, 2, 4)}
        old |= {f'host-{mode}-{backend}-{width}' for mode in ('success', 'domain', 'cancel')
                for backend in ('llvm', 'cranelift') for width in (1, 2, 4)}
        source = (ROOT / 'crates/hirsel-cli/tests/cli/pubsub_relay_30_cells.rs').read_text()
        registrations = re.findall(r'#\[test\]\s+fn (\w+)\(\) \{\s+super::pubsub_relay_30::native_cell\("([^"]+)"\);\s+}', source)
        self.assertEqual(len(registrations), 126)
        self.assertEqual(len({name for name, _ in registrations}), 126)
        self.assertEqual({cell for _, cell in registrations}, old)
        self.assertEqual(set(selection.inventory(CASES)), old)
        for name, cell in registrations:
            self.assertEqual(name, cell.replace('-', '_'))
            self.assertEqual(selection.select_cells(CASES, (cell,)), (cell,))
            self.assertEqual(selection.required_receipts((cell,)), {
                'llvm-build', 'cranelift-build', cell,
                cell + '-replay-llvm', cell + '-replay-cranelift'})
        rust = (ROOT / 'crates/hirsel-cli/tests/cli/pubsub_relay_30.rs').read_text()
        self.assertIn('[false, true].into_iter()', rust.replace('\n', '').replace('        ', ''))
        self.assertIn('replay_observations_checked(', rust)
        self.assertRegex(rust, r'replay_loaded_observations\(\s*&mut program,\s*&path,\s*'
                         r'Some\(10_000_000\),\s*checked,\s*\)')

    def test_invalid_empty_duplicate_selection_and_missing_receipts(self):
        for requested in [(), ('unknown',), ('denied-llvm-3',), ('denied-llvm-1',) * 2]:
            with self.subTest(requested=requested), self.assertRaises(AssertionError):
                selection.select_cells(CASES, requested)
        cell = ('denied-llvm-1',)
        rows = [{'id': name, 'passed': True, 'status': 0}
                for name in selection.required_receipts(cell)]
        selection.check_receipts(cell, rows)
        for index in range(len(rows)):
            with self.subTest(missing=rows[index]['id']), self.assertRaises(AssertionError):
                selection.check_receipts(cell, rows[:index] + rows[index + 1:])
        for changed in [rows + [rows[0]], rows + [{'id': 'unknown', 'passed': True}],
                        [dict(row, passed=False) for row in rows]]:
            with self.assertRaises(AssertionError):
                selection.check_receipts(cell, changed)

    def dispatch(self, cell, failure=None):
        with tempfile.TemporaryDirectory() as scratch:
            root = Path(scratch)
            for name in ['verify.py', 'host.py', 'model.py', 'selection.py']:
                shutil.copyfile(SUPPORT / name, root / name)
            (root / 'main.hir').write_text('frozen app stand-in')
            (root / 'hirsel.json').write_text('{}')
            (root / 'cases.json').write_text(json.dumps(CASES))
            (root / 'capped').write_text('fake capped')
            tools = root / 'tools'
            (tools / 'bin').mkdir(parents=True)
            (tools / 'helper').write_text('frozen tool bytes')
            cli = root / 'cli'
            cli.write_text('fake CLI bytes')
            environ = {key: str(tools) for key in ('HIRSEL_PRODUCTS', 'LLVM_SYS_231_PREFIX',
                                                 'HIRSEL_RUNTIME_ARCHIVES', 'HIRSEL_LINK_KIT')}
            environ.update(PUBSUB_30_ENGINE='deliberately-invalid', PUBSUB_30_CELL='unknown')
            calls, outputs = [], {}

            def fake_run(argv, **kwargs):
                calls.append(argv)
                if 'build' in argv:
                    backend = argv[argv.index('--backend') + 1]
                    if failure == 'build-' + backend:
                        return subprocess.CompletedProcess(argv, 1, b'', b'fake build failure')
                    Path(argv[-1]).write_text('fake executable')
                    return subprocess.CompletedProcess(argv, 0, b'', b'')
                self.assertEqual(argv[1:5], ['--timeout', '15', 'prlimit', '--cpu=60:61'])
                self.assertEqual(kwargs['timeout'], 20)
                if '--hirsel-record' in argv:
                    if failure == 'timeout':
                        raise subprocess.TimeoutExpired(argv, 20, output=b'partial')
                    tape = Path(argv[argv.index('--hirsel-record') + 1])
                    case_id = tape.stem.rsplit('-', 2)[0]
                    case = next(case for case in CASES if case['id'] == case_id)
                    if case['exit']:
                        out = json.dumps({'error': 'usage_or_limit' if case['exit'] == 2 else 'io'}).encode() + b'\n'
                    else:
                        out = json.dumps(dict(joined=True, credits_held=0, reserved=1, returned=1,
                                              queued=case['queued'], written=case['queued'],
                                              disconnected=0, admitted=case['admitted'], reason=case['reason'])).encode() + b'\n'
                    outputs[tape] = case['exit'], out
                    if failure != 'missing-tape':
                        tape.write_bytes(b'fake immutable tape')
                    if failure == 'child':
                        return subprocess.CompletedProcess(argv, 137, b'', b'fake child failure')
                    if failure == 'partial-stdout':
                        out = out[:len(out) // 2]
                    return subprocess.CompletedProcess(argv, case['exit'], out, b'')
                tape = Path(argv[-1])
                backend = Path(argv[5]).name
                code, out = outputs[tape]
                if failure == 'replay-' + backend:
                    code = 137
                if failure == 'partial-replay':
                    out = out[:-2]
                return subprocess.CompletedProcess(argv, code, out, b'')

            def fake_host(exe, destination, width, mode, env, capped):
                self.assertEqual(cell, f'host-{mode}-{exe.name}-{width}')
                tape = destination / (cell + '.tape')
                tape.write_bytes(b'fake immutable host tape')
                out = b'fake host stdout\n'
                outputs[tape] = 0, out
                return dict(id=cell, tape=str(tape), stdout=out.decode(), passed=True)

            with patch.dict(os.environ, environ), patch.object(verify.subprocess, 'run', fake_run), \
                 patch.object(verify, 'host_case', fake_host), contextlib.redirect_stdout(io.StringIO()):
                result = verify.main(str(cli), str(root / 'hirsel.json'), (cell,))
            self.assertEqual(result, 0)
            rows = json.loads((root / 'receipts.json').read_text())
            self.assertEqual({row['id'] for row in rows}, selection.required_receipts((cell,)))
            self.assertEqual(sum('--hirsel-record' in argv for argv in calls), int(not cell.startswith('host-')))
            self.assertEqual(sum('--hirsel-replay' in argv for argv in calls), 2)

    def test_all_126_default_bodies_dispatch_exactly_one_recording_and_both_native_replayers(self):
        for cell in selection.inventory(CASES):
            with self.subTest(cell=cell):
                self.dispatch(cell)

    def test_inner_failures_cannot_pass_the_owning_partition(self):
        for failure in ['build-llvm', 'build-cranelift', 'child', 'timeout', 'missing-tape',
                        'replay-llvm', 'replay-cranelift', 'partial-stdout', 'partial-replay']:
            with self.subTest(failure=failure), self.assertRaises((AssertionError, OSError)):
                self.dispatch('denied-llvm-1', failure)
        for backend in ('llvm', 'cranelift'):
            with self.subTest(host=backend), self.assertRaises(AssertionError):
                self.dispatch('host-domain-cranelift-2', 'replay-' + backend)


if __name__ == '__main__':
    unittest.main()
