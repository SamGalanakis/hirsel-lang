"""Native reports retain physical payload classes and distinct RC operations.

Known file-digest controls adapted from Go cache/hash_test.go:13-51.

Copyright 2017 The Go Authors.

Redistribution and use in source and binary forms, with or without
modification, are permitted provided that the following conditions are
met:

   * Redistributions of source code must retain the above copyright
notice, this list of conditions and the following disclaimer.
   * Redistributions in binary form must reproduce the above
copyright notice, this list of conditions and the following disclaimer
in the documentation and/or other materials provided with the
distribution.
   * Neither the name of Google LLC nor the names of its
contributors may be used to endorse or promote products derived from
this software without specific prior written permission.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS
"AS IS" AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT
LIMITED TO, THE IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR
A PARTICULAR PURPOSE ARE DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT
OWNER OR CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL,
SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT
LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES; LOSS OF USE,
DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON ANY
THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT
(INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE
OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.
"""
import copy
import importlib.machinery
import importlib.util
import json
import sys
import tempfile
from unittest.mock import patch
import unittest
from pathlib import Path
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import suite
from suite import ROOT, current_manifest, freeze, load_manifest, parse_measurement, sha

REPORT = '''hirsel-measurement 1
class 8 2 2
class large 1 1
count_increments 3
count_decrements 5
cow_detaches 0
deep_copies 0
copied_blocks 0
copied_bytes 0
reuse_hits 1
reuse_misses 2
live_bytes 0
peak_live_bytes 72
region_high_water_bytes 0
region_pool_high_water_bytes 0
arena_high_water_bytes unavailable
'''

class NativeMeasurement(unittest.TestCase):
    def test_preserves_units_and_unavailable_arena(self):
        row = parse_measurement(REPORT)
        self.assertEqual(row['classes']['8'], {'allocations': 2, 'frees': 2})
        self.assertEqual(row['count_increments'], 3)
        self.assertEqual(row['count_decrements'], 5)
        self.assertEqual(row['arena_high_water_bytes'], 'unavailable')
        self.assertNotIn('arena_peak_reserved_bytes', row)

    def test_incomplete_duplicate_negative_or_wrong_version_refuses(self):
        for report in (REPORT.replace('hirsel-measurement 1', 'hirsel-measurement 2'),
                       REPORT.replace('copied_bytes 0\n', ''),
                       REPORT + 'live_bytes 0\n', REPORT + 'class 8 1 1\n',
                       REPORT.replace('reuse_hits 1', 'reuse_hits -1'),
                       REPORT.replace('class large 1 1', 'class large -1 1'),
                       REPORT.replace('arena_high_water_bytes unavailable', 'arena_high_water_bytes 0')):
            with self.subTest(report=report), self.assertRaises(ValueError):
                parse_measurement(report)


class ManifestSeals(unittest.TestCase):
    def test_file_hash_has_an_independent_known_digest(self):
        # Go cache/hash_test.go:13-51 uses this published SHA-256 vector to
        # check the digest independently of the file hashing implementation.
        with tempfile.TemporaryDirectory() as tmp:
            source = Path(tmp) / 'source'
            source.write_bytes(b'hello world')
            self.assertEqual(sha(source),
                             'b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9')
            source.write_bytes(b'')
            self.assertEqual(sha(source),
                             'e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855')

    def test_current_seal_preserves_the_contract_and_historical_file(self):
        before = (ROOT / 'manifest.json').read_bytes()
        old = json.loads(before)
        fresh = load_manifest(current_manifest())
        self.assertEqual({k: v for k, v in fresh.items() if k != 'source_sha256'},
                         {k: v for k, v in old.items() if k != 'source_sha256'})
        self.assertEqual(set(fresh['source_sha256']), set(suite.SOURCE_FILES))
        self.assertEqual(fresh['source_sha256']['../../crates/hirsel-native-build/src/main.rs'],
                         sha(ROOT / '../../crates/hirsel-native-build/src/main.rs'))
        fresh['cases'][0]['checksum'] = 'changed'
        self.assertEqual((ROOT / 'manifest.json').read_bytes(), before)
        self.assertNotEqual(current_manifest()['cases'][0]['checksum'], 'changed')

    def test_historical_manifest_and_stale_helper_still_refuse(self):
        old = json.loads((ROOT / 'manifest.json').read_bytes())
        with self.assertRaisesRegex(AssertionError, 'source changed'):
            load_manifest()
        helper = '../../crates/hirsel-native-build/src/main.rs'
        self.assertNotEqual(old['source_sha256'][helper], sha(ROOT / helper))
        fresh = current_manifest()
        fresh['source_sha256'][helper] = old['source_sha256'][helper]
        with self.assertRaisesRegex(AssertionError, 'source changed: ../../crates/hirsel-native-build/src/main.rs'):
            load_manifest(fresh)

    def assert_contract_refuses(self, manifest, message):
        # A private template copy tests construction without changing repository
        # fixtures. Failure must precede source hashing, even with a fresh seal.
        with self.assertRaisesRegex(AssertionError, message):
            load_manifest(manifest)
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / 'generate').symlink_to(ROOT / 'generate')
            (root / 'inputs').symlink_to(ROOT / 'inputs')
            (root / 'manifest.json').write_text(json.dumps(manifest))
            with patch.object(suite, 'ROOT', root), self.assertRaisesRegex(AssertionError, message):
                current_manifest()

    def test_changed_input_and_oracle_bytes_refuse_independently(self):
        fresh = current_manifest()
        with tempfile.TemporaryDirectory() as tmp:
            for field in ('input', 'oracle'):
                with self.subTest(field=field):
                    broken = copy.deepcopy(fresh)
                    changed = Path(tmp) / field
                    changed.write_bytes((ROOT / broken['cases'][0][field]).read_bytes() + b'changed')
                    broken['cases'][0][field] = str(changed)
                    self.assert_contract_refuses(broken, 'fixture changed')

    def test_changed_checksum_refuses_independently(self):
        broken = current_manifest()
        broken['cases'][0]['checksum'] = '0' * 16
        self.assert_contract_refuses(broken, 'fixture checksum changed')

    def test_missing_participant_case_and_source_refuse(self):
        fresh = current_manifest()
        for field, message in (('languages', 'missing reference'), ('variants', 'missing reference'),
                               ('required_targets', 'missing required target'), ('cases', 'missing case'),
                               ('source_sha256', 'missing source')):
            with self.subTest(field=field):
                broken = copy.deepcopy(fresh)
                if isinstance(broken[field], dict):
                    broken[field].pop(next(iter(broken[field])))
                else:
                    broken[field].pop()
                self.assert_contract_refuses(broken, message)

    def test_external_freeze_roundtrips_without_overwriting_history(self):
        before = (ROOT / 'manifest.json').read_bytes()
        with tempfile.TemporaryDirectory() as tmp:
            output = Path(tmp) / 'manifest.json'
            frozen = freeze(output)
            self.assertEqual(load_manifest(json.loads(output.read_bytes())), frozen)
            with self.assertRaises(FileExistsError):
                freeze(output)
        for output in (ROOT / 'manifest.json', ROOT / 'new-manifest.json', Path('manifest.json')):
            with self.subTest(output=output), self.assertRaisesRegex(ValueError, 'outside the repository'):
                freeze(output)
        self.assertEqual((ROOT / 'manifest.json').read_bytes(), before)

    def test_runner_retains_supplied_seal_and_refuses_stale_reuse(self):
        loader = importlib.machinery.SourceFileLoader('seal_runner', str(ROOT / 'run'))
        spec = importlib.util.spec_from_loader(loader.name, loader)
        runner = importlib.util.module_from_spec(spec)
        loader.exec_module(runner)
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            manifest_path = root / 'manifest.json'
            manifest = freeze(manifest_path)
            binary = root / 'binary'
            binary.write_bytes(b'sealed executable')
            build = {'manifest_sha256': sha(manifest_path), 'source_sha256': manifest['source_sha256'],
                     'executables': {'c/arena': {'path': str(binary), 'sha256': sha(binary)}}}
            build_path = root / 'build.json'
            output = root / 'verification.json'
            argv = ['run', '--manifest', str(manifest_path), '--reuse-build', str(build_path),
                    '--verify', '--output', str(output)]
            correctness = {'checks': 12, 'failures': [], 'passed': True}
            with patch.object(sys, 'argv', argv), patch.object(runner, 'verify', return_value=correctness) as verify:
                build_path.write_text(json.dumps(build))
                runner.main()
                result = json.loads(output.read_bytes())
                self.assertEqual(result['manifest_sha256'], sha(manifest_path))
                self.assertEqual(result['manifest'], manifest)
                self.assertEqual(result['build'], build)
                verify.assert_called_once_with(manifest, build, None)
                for field, message in (('manifest_sha256', 'build manifest changed'),
                                       ('source_sha256', 'build source changed')):
                    with self.subTest(field=field):
                        stale = dict(build, **{field: 'stale'})
                        build_path.write_text(json.dumps(stale))
                        with self.assertRaisesRegex(AssertionError, message):
                            runner.main()
                build_path.write_text(json.dumps(build))
                binary.write_bytes(b'changed executable')
                with self.assertRaisesRegex(AssertionError, 'build binary changed'):
                    runner.main()
                self.assertEqual(verify.call_count, 1)

    def test_runner_freeze_requires_an_explicit_external_destination(self):
        loader = importlib.machinery.SourceFileLoader('freeze_runner', str(ROOT / 'run'))
        spec = importlib.util.spec_from_loader(loader.name, loader)
        runner = importlib.util.module_from_spec(spec)
        loader.exec_module(runner)
        with tempfile.TemporaryDirectory() as tmp:
            output = Path(tmp) / 'manifest.json'
            for args in (['--freeze'], ['--freeze-output', str(output)]):
                with self.subTest(args=args), patch.object(sys, 'argv', ['run', *args]), \
                     patch('sys.stderr'), self.assertRaises(SystemExit) as error:
                    runner.main()
                self.assertEqual(error.exception.code, 2)
                self.assertFalse(output.exists())
            with patch.object(sys, 'argv', ['run', '--freeze', '--freeze-output', str(output)]):
                runner.main()
            self.assertEqual(load_manifest(json.loads(output.read_bytes())), current_manifest())
            # Strict loading happens before any build or verification can begin.
            with patch.object(sys, 'argv', ['run', '--manifest', str(ROOT / 'manifest.json'), '--build-only']), \
                 patch.object(runner, 'build') as build, self.assertRaisesRegex(AssertionError, 'source changed'):
                runner.main()
            build.assert_not_called()
