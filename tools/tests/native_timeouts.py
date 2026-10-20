"""A failed remote diagnostic still publishes its launch artifacts and verdict."""
import json
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]


class NativeTimeouts(unittest.TestCase):
    def test_failed_campaign_keeps_verdict_sources_and_scratch(self):
        with tempfile.TemporaryDirectory() as tmp:
            tmp = Path(tmp)
            corpus = tmp / 'corpus'
            (corpus / 'native-tests/regressions').mkdir(parents=True)
            (corpus / 'conformance/SEM').mkdir(parents=True)
            for case in ('crlf', 'explicit'):
                source = corpus / f'conformance/SEM/SEM-3-yaml-edit-{case}-run.hir'
                source.write_text(case)
                source.with_name(source.name + '.json').write_text(json.dumps({
                    'sources': [{'path': source.name, 'module': []}],
                    'dependencies': [{'manifest': '../../stdlib/std@1/hirsel.json'}],
                }))
            native = tmp / 'native'
            native.write_text('''#!/usr/bin/python3
import os, pathlib, sys
assert sys.argv[sys.argv.index('--jobs') + 1] == '2'
assert os.environ['HIRSEL_JOBS'] == '2'
root = pathlib.Path(sys.argv[sys.argv.index('--root') + 1])
import json
for sidecar in (root / 'native-tests/regressions').glob('*.hir.json'):
    manifest = json.loads(sidecar.read_text())
    assert manifest['sources'][0]['path'] == sidecar.name.removesuffix('.json')
    assert manifest['dependencies'][0]['manifest'] == '../../stdlib/std@1/hirsel.json'
scratch = pathlib.Path(sys.argv[0]).parent / 'native-gate'
scratch.mkdir()
(scratch / 'program').write_text('retained executable')
print('launch timeout')
sys.exit(1)
''')
            native.chmod(0o755)
            output = tmp / 'output'
            command = ['python3', str(ROOT / 'tools/buck2/native_timeouts.py'), '--native', str(native),
                       '--root', str(corpus), '--output', str(output), '--repetitions', '2']
            for name in ('llvm', 'kit', 'glibc_floor', 'archives'):
                command += ['--tool', name, str(tmp)]
            subprocess.run(command, check=True, capture_output=True)
            result = json.loads((output / 'result.json').read_text())
            self.assertEqual((result['exit_code'], result['cases'], result['jobs']), (1, 4, 2))
            self.assertEqual(len(list((output / 'cases').glob('*.hir'))), 4)
            self.assertEqual(len(list((output / 'cases').glob('*.hir.json'))), 4)
            self.assertEqual((output / 'native-gate/program').read_text(), 'retained executable')
            self.assertIn('launch timeout', (output / 'campaign.log').read_text())
