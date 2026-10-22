#!/usr/bin/env python3
"""Authoring controls and generated-source checks against the existing hooks."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
COMPILER = None
if '--hirsel' in sys.argv:
    index = sys.argv.index('--hirsel')
    COMPILER = Path(sys.argv[index + 1]).resolve()
    del sys.argv[index:index + 2]


def generate(*args):
    return subprocess.run([sys.executable, str(ROOT / 'tools/json-hooks'), *args],
                          capture_output=True, text=True)


class Unit(unittest.TestCase):
    def bad(self, *args, message):
        result = generate(*args)
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(result.stdout, '')
        self.assertIn(message, result.stderr)

    def test_invalid_identifiers(self):
        for name in ['3Item', 'Item-name', 'I tém', 'Éclair', '_', 'fn', 'shape', 'true', 'Item\nOther']:
            with self.subTest(name=name):
                self.bad('Snapshot', name, '--root', 'Snapshot', message='local non-generic nominal')

    def test_imported_and_generic_spelling(self):
        for name in ['lib.Item', 'lib@1/Item', 'lib/Item', 'Item[u64]', 'Item<T>', '(Item,Item)']:
            with self.subTest(name=name):
                self.bad(name, '--root', name, message='wire imported hooks manually')

    def test_builtins(self):
        for name in ['Date', 'Option', 'List', 'String', 'Bytes', 'u64']:
            with self.subTest(name=name):
                self.bad('Snapshot', name, '--root', 'Snapshot', message='built-in types')

    def test_required_arguments(self):
        self.bad('Item', message='--root')
        self.bad('--root', 'Item', message='TYPE')
        self.bad('Item', '--root', message='expected one argument')
        self.bad('Item', '--root', 'Item', '--unexpected', message='unrecognized arguments')
        self.bad('Item', '--roo', 'Item', message='--root')

    def test_root_must_be_listed_and_valid(self):
        self.bad('Item', '--root', 'Snapshot', message='must be included')
        self.bad('Item', '--root', 'bad.Root', message='local non-generic nominal')

    def test_duplicates_and_colliding_stems(self):
        for names in [('Item', 'Item'), ('HTTPServer', 'HttpServer'), ('Foo_Bar', 'FooBar'), ('Foo', 'foo')]:
            with self.subTest(names=names):
                self.bad(*names, '--root', names[0], message='colliding generated stem')

    def test_generated_symbol_collisions(self):
        for name in ['item_from_json_reader', 'item_json_into', 'item_from_json_bytes']:
            self.bad('Item', name, '--root', 'Item', message='collides with a supplied type')

    def test_acronyms_underscores_and_digits(self):
        names = [('HTTPServer', 'http_server'), ('XML', 'xml'), ('ABc', 'a_bc'),
                 ('A1B', 'a1_b'), ('Foo_Bar', 'foo_bar'), ('_Item', '_item'), ('item2', 'item2')]
        result = generate(*(n for n, _ in names), '--root', 'XML')
        self.assertEqual(result.returncode, 0, result.stderr)
        for _, stem in names:
            self.assertIn(f'fn {stem}_from_json_reader(', result.stdout)
            self.assertIn(f'fn {stem}_json_into(', result.stdout)
        self.assertEqual(result.stdout.count('_from_json_bytes('), 1)

    def test_exact_single_type_output(self):
        expected = '''fn item_from_json_reader(mut cursor: std@1/json.Reader) -> core@1/Result[Item, core@1/String] uses {} = comptime std@1/derive.from_json_reader(
  shape(Item)
)

fn item_json_into(value: Item, mut out: core@1/List[core@1/u8], depth: core@1/u64) -> core@1/Result[
  (),
  std@1/json.JsonError
] uses {} = comptime std@1/derive.json_into(shape(Item))

fn item_from_json_bytes(data: core@1/Bytes) -> core@1/Result[Item, core@1/String] uses {} = comptime std@1/derive.from_json_bytes(
  shape(Item)
)
'''
        result = generate('Item', '--root', 'Item')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stderr, '')
        self.assertEqual(result.stdout, expected)

    def test_deterministic_complete_output(self):
        first = generate('Item', 'Snapshot', '--root', 'Snapshot')
        second = generate('--root', 'Snapshot', 'Item', 'Snapshot')
        self.assertEqual(first.returncode, 0, first.stderr)
        self.assertEqual(first.stdout, second.stdout)
        self.assertEqual(first.stdout.count('\nfn ') + first.stdout.startswith('fn '), 5)
        self.assertLess(first.stdout.index('fn item_'), first.stdout.index('fn snapshot_'))
        self.assertNotIn('comptime derive.', first.stdout)

    def test_errors_do_not_modify_files(self):
        with tempfile.TemporaryDirectory() as temporary:
            sentinel = Path(temporary) / 'keep.hir'
            sentinel.write_text('user source\n')
            result = subprocess.run([sys.executable, str(ROOT / 'tools/json-hooks'), 'Item',
                                     str(sentinel), '--root', 'Item'], cwd=temporary,
                                    capture_output=True, text=True)
            self.assertNotEqual(result.returncode, 0)
            self.assertEqual(result.stdout, '')
            self.assertEqual(sentinel.read_text(), 'user source\n')
            self.assertEqual(list(Path(temporary).iterdir()), [sentinel])


class Integration(unittest.TestCase):
    def test_missing_nested_hooks_then_complete_graph_on_three_engines(self):
        self.assertIsNotNone(COMPILER, 'integration requires --hirsel PATH; use the Buck2 target')
        with tempfile.TemporaryDirectory(prefix='typed-json-kit-') as temporary:
            package = Path(temporary)
            fixture = ROOT / 'examples/typed-json-kit'
            (package / 'main.hir').write_bytes((fixture / 'main.hir').read_bytes())
            manifest = json.loads((fixture / 'hirsel.json').read_text())
            manifest['dependencies'][0]['manifest'] = str(ROOT / 'stdlib/std@1/hirsel.json')
            path = package / 'hirsel.json'
            path.write_text(json.dumps(manifest))
            hooks = package / 'hooks.hir'
            incomplete = generate('Snapshot', '--root', 'Snapshot')
            self.assertEqual(incomplete.returncode, 0, incomplete.stderr)
            hooks.write_text(incomplete.stdout)
            env = os.environ | {'HIRSEL_DAEMON': 'off', 'HIRSEL_RESULTS': 'none@1', 'HIRSEL_VALIDATE_STAGES': '1'}
            missing = subprocess.run([str(COMPILER), 'test', '--json', str(path)], env=env,
                                     capture_output=True, text=True)
            self.assertNotEqual(missing.returncode, 0, missing.stdout + missing.stderr)
            self.assertRegex(missing.stdout + missing.stderr, r'item_(from_json_reader|json_into)')
            print('negative control: missing Item hooks rejected', flush=True)
            complete = generate('Item', 'Snapshot', '--root', 'Snapshot')
            self.assertEqual(complete.returncode, 0, complete.stderr)
            hooks.write_text(complete.stdout)
            launch = json.loads((fixture / 'check.json').read_text())
            launch['dependencies'] = manifest['dependencies']
            (package / 'check.hir').write_bytes((fixture / 'check.hir').read_bytes())
            check = package / 'check.json'
            check.write_text(json.dumps(launch))
            checked = subprocess.run([str(COMPILER), 'check', str(check)], env=env,
                                     capture_output=True, text=True)
            self.assertEqual(checked.returncode, 0, checked.stdout + checked.stderr)
            expected = {'nested_roundtrip', 'retained_after_input_drop', 'unknown_nested_values', 'malformed_nested_values'}
            for engine, args in [('H', []), ('LLVM', ['--native', '--backend', 'llvm']),
                                 ('Cranelift', ['--native', '--backend', 'cranelift'])]:
                with self.subTest(engine=engine):
                    result = subprocess.run([str(COMPILER), 'test', '--json', str(path), *args],
                                            env=env, capture_output=True, text=True)
                    self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                    report = json.loads(result.stdout)
                    self.assertEqual(report['summary'], {'passed': 4, 'failed': 0, 'total': 4})
                    self.assertEqual({test['name'].split('/')[-1] for test in report['tests']}, expected)
                    self.assertTrue(all(test['status'] == 'passed' for test in report['tests']))
                    print(f'{engine}: 4 selected, 4 passed; exact fixture inventory checked', flush=True)


if __name__ == '__main__':
    if len(sys.argv) == 1:
        sys.argv.append('Unit')
    unittest.main(verbosity=2)
