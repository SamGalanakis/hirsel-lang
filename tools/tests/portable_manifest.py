#!/usr/bin/env python3
"""Checked local-path rewrites and an isolated, relocated package on H/Cranelift."""
import copy
import json
import ntpath
import os
from pathlib import Path
import runpy
import shutil
import stat
import subprocess
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
TOOL = ROOT / 'tools/portable-manifest'
API = runpy.run_path(str(TOOL))
COMPILER = None
if '--hirsel' in sys.argv:
    index = sys.argv.index('--hirsel')
    COMPILER = Path(sys.argv[index + 1]).resolve()
    del sys.argv[index:index + 2]


def package(name='app', dependencies=None, **extra):
    return dict(schema=1, package=dict(name=name, version='1'),
                sources=[dict(path='./main.hir', module=[])],
                dependencies=dependencies or [], **extra)


def dependency(path, name='lib', version='1', **extra):
    return dict(manifest=str(path), package=dict(name=name, version=version), expose=[], **extra)


def save(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, ensure_ascii=False) + '\n', encoding='utf-8')
    return path


def rewrite(manifest, root, *args, cwd=None):
    return subprocess.run([sys.executable, str(TOOL), str(manifest), '--workspace-root',
                           str(root), *map(str, args)], cwd=cwd, capture_output=True)


class Unit(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix='portable-manifest-8-')
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name) / 'workspace'
        self.lib = save(self.root / 'library/hirsel.json', package('lib'))
        self.manifest = save(self.root / 'app/hirsel.json', package(dependencies=[dependency(self.lib)]))

    def success(self, *args, manifest=None, cwd=None):
        path = manifest or self.manifest
        before = path.read_bytes()
        result = rewrite(path, self.root, *args, cwd=cwd)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stderr, b'')
        if '--write' not in args:
            self.assertEqual(path.read_bytes(), before)
            return json.loads(result.stdout)
        self.assertEqual(result.stdout, b'')
        return json.loads(path.read_bytes())

    def refused(self, message, *args, manifest=None):
        path = manifest or self.manifest
        before = path.read_bytes()
        files = sorted(self.root.rglob('*'))
        for mode in [[], ['--write']]:
            with self.subTest(mode=mode):
                result = rewrite(path, self.root, *args, *mode)
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual(result.stdout, b'')
                self.assertIn(message.encode(), result.stderr)
                self.assertEqual(path.read_bytes(), before)
                self.assertEqual(sorted(self.root.rglob('*')), files)

    def test_absolute_and_relative_dependencies_use_manifest_directory(self):
        second = save(self.root / 'other/hirsel.json', package('other'))
        save(self.manifest, package(dependencies=[dependency(self.lib),
                                                 dependency('../other/./hirsel.json', 'other')]))
        result = self.success(cwd=self.root / 'library')
        self.assertEqual([d['manifest'] for d in result['dependencies']],
                         ['../library/hirsel.json', '../other/hirsel.json'])

    def test_spaces_unicode_sibling_and_ancestor(self):
        ancestor = save(self.root / 'hirsel.json', package('ancestor'))
        special = save(self.root / 'lib space 雪/hirsel.json', package('special'))
        save(self.manifest, package(dependencies=[dependency(ancestor, 'ancestor'),
                                                 dependency(special, 'special')]))
        result = self.success()
        self.assertEqual([d['manifest'] for d in result['dependencies']],
                         ['../hirsel.json', '../lib space 雪/hirsel.json'])

    def test_unknown_fields_sources_entry_and_grants_preserved(self):
        value = package(dependencies=[dependency(self.lib, note={'future': [1, False, None]})],
                        entry={'module': [], 'name': 'main'}, grants='../odd spelling/./grants.json',
                        future={'unicode': '雪', 'number': 1.25})
        value['sources'] = [{'path': '../sources/./a b 雪.hir', 'module': ['module'], 'future': 7}]
        value['package']['future'] = 'preserved'
        save(self.manifest, value)
        expected = copy.deepcopy(value)
        expected['dependencies'][0]['manifest'] = '../library/hirsel.json'
        self.assertEqual(self.success(), expected)

    def test_deterministic_pretty_json_and_idempotence(self):
        first = rewrite(self.manifest, self.root)
        self.assertEqual(first.returncode, 0, first.stderr)
        self.assertTrue(first.stdout.endswith(b'\n'))
        self.assertIn(b'\n  "dependencies": [\n', first.stdout)
        self.manifest.write_bytes(first.stdout)
        second = rewrite(self.manifest, self.root)
        self.assertEqual(second.returncode, 0, second.stderr)
        self.assertEqual(first.stdout, second.stdout)

    def test_write_preserves_permissions_and_only_replaces_input(self):
        self.manifest.chmod(0o640)
        before = self.lib.read_bytes()
        files = sorted(self.root.rglob('*'))
        self.assertEqual(self.success('--write')['dependencies'][0]['manifest'], '../library/hirsel.json')
        self.assertEqual(stat.S_IMODE(self.manifest.stat().st_mode), 0o640)
        self.assertEqual(self.lib.read_bytes(), before)
        self.assertEqual(sorted(self.root.rglob('*')), files)

    def test_atomic_replace_failure_cleans_temporary(self):
        before = self.manifest.read_bytes()
        files = sorted(self.manifest.parent.iterdir())
        function = API['replace_manifest']
        with patch.object(os, 'replace', side_effect=OSError('injected replace failure')):
            with self.assertRaisesRegex(OSError, 'injected'):
                function(self.manifest, b'new bytes')
        self.assertEqual(self.manifest.read_bytes(), before)
        self.assertEqual(sorted(self.manifest.parent.iterdir()), files)

    def test_explicit_std_replaces_stale_external_path_only(self):
        std = save(self.root / 'stdlib/std@1/hirsel.json', package('std'))
        value = package(dependencies=[dependency('/unavailable/toolchain/std/hirsel.json', 'std'),
                                      dependency(self.lib)])
        save(self.manifest, value)
        expected = copy.deepcopy(value)
        expected['dependencies'][0]['manifest'] = '../stdlib/std@1/hirsel.json'
        expected['dependencies'][1]['manifest'] = '../library/hirsel.json'
        self.assertEqual(self.success('--std-manifest', std), expected)

    def test_std_already_in_workspace_needs_no_override(self):
        std = save(self.root / 'stdlib/hirsel.json', package('std'))
        save(self.manifest, package(dependencies=[dependency(std, 'std')]))
        self.assertEqual(self.success()['dependencies'][0]['manifest'], '../stdlib/hirsel.json')

    def test_missing_std_selection_does_not_infer_from_path(self):
        std = save(self.root / 'std@1/hirsel.json', package('std'))
        self.refused('requires exactly one existing std@1', '--std-manifest', std)

    def test_duplicate_std_is_ambiguous_even_without_override(self):
        std = save(self.root / 'stdlib/hirsel.json', package('std'))
        save(self.manifest, package(dependencies=[dependency(std, 'std'), dependency(std, 'std')]))
        self.refused('ambiguous duplicate std@1')
        self.refused('ambiguous duplicate std@1', '--std-manifest', std)

    def test_wrong_std_identity_and_version(self):
        for name, version in [('lib', '1'), ('std', '2')]:
            with self.subTest(name=name, version=version):
                std_value = package(name)
                std_value['package']['version'] = version
                std = save(self.root / 'stdlib/hirsel.json', std_value)
                save(self.manifest, package(dependencies=[dependency('stale.json', 'std')]))
                self.refused('target must identify std@1', '--std-manifest', std)

    def test_missing_std_replacement(self):
        save(self.manifest, package(dependencies=[dependency('stale.json', 'std')]))
        self.refused('No such file', '--std-manifest', self.root / 'missing.json')

    def test_missing_dependency_after_valid_dependency_has_no_partial_output(self):
        save(self.manifest, package(dependencies=[dependency(self.lib), dependency('missing.json')]))
        self.refused('No such file')

    def test_dependency_identity_mismatch(self):
        save(self.manifest, package(dependencies=[dependency(self.lib, 'other')]))
        self.refused('identity does not match')

    def test_outside_root_and_shared_prefix_are_rejected(self):
        outside = save(self.root.with_name('workspace-other') / 'hirsel.json', package('lib'))
        save(self.manifest, package(dependencies=[dependency(outside)]))
        self.refused('outside workspace root')

    def test_outside_std_replacement_is_rejected(self):
        outside = save(self.root.parent / 'std.json', package('std'))
        save(self.manifest, package(dependencies=[dependency('stale.json', 'std')]))
        self.refused('outside workspace root', '--std-manifest', outside)

    def test_package_itself_must_be_inside_root(self):
        outside = save(self.root.parent / 'outside.json', package())
        self.refused('outside workspace root', manifest=outside)

    def test_escaped_file_and_directory_symlinks(self):
        outside = save(self.root.parent / 'outside/hirsel.json', package('lib'))
        (self.root / 'file.json').symlink_to(outside)
        (self.root / 'directory').symlink_to(outside.parent, target_is_directory=True)
        for path in [self.root / 'file.json', self.root / 'directory/hirsel.json']:
            save(self.manifest, package(dependencies=[dependency(path)]))
            self.refused('outside workspace root')

    def test_contained_symlink_uses_resolved_layout(self):
        alias = self.root / 'alias'
        alias.symlink_to(self.lib.parent, target_is_directory=True)
        save(self.manifest, package(dependencies=[dependency('../alias/hirsel.json')]))
        self.assertEqual(self.success()['dependencies'][0]['manifest'], '../library/hirsel.json')
        app_alias = self.root / 'app_alias'
        app_alias.symlink_to(self.manifest.parent, target_is_directory=True)
        self.assertEqual(self.success(manifest=app_alias / 'hirsel.json'), self.success())

    def test_symlink_input_dry_output_but_write_refused(self):
        alias = self.root / 'alias.json'
        alias.symlink_to(self.manifest)
        self.assertEqual(self.success(manifest=alias), self.success())
        before = self.manifest.read_bytes()
        result = rewrite(alias, self.root, '--write')
        self.assertNotEqual(result.returncode, 0)
        self.assertIn(b'refuses a symlink', result.stderr)
        self.assertEqual(result.stdout, b'')
        self.assertTrue(alias.is_symlink())
        self.assertEqual(self.manifest.read_bytes(), before)

    def test_unknown_schema_and_malformed_shapes(self):
        for field, value in [('schema', 3), ('schema', True), ('schema', '1'),
                             ('dependencies', {}), ('dependencies', ['remote']),
                             ('dependencies', [{'url': 'https://example.com', 'package': {'name': 'lib', 'version': '1'}}]),
                             ('sources', [])]:
            with self.subTest(field=field, value=value):
                document = package()
                document[field] = value
                save(self.manifest, document)
                self.refused('error:')

    def test_remote_and_nonstring_references_are_rejected(self):
        for value in ['https://example.com/hirsel.json', 'git+https://example.com/repo',
                      'file:///tmp/hirsel.json', '//host/share/hirsel.json', r'C:\std\hirsel.json',
                      r'\\host\share\hirsel.json', '', None, {'path': '../library/hirsel.json'}]:
            with self.subTest(value=value):
                document = package(dependencies=[dependency(self.lib)])
                document['dependencies'][0]['manifest'] = value
                save(self.manifest, document)
                self.refused('local filesystem path')

    def test_invalid_package_identities(self):
        for name, version in [('fn', '1'), ('é', '1'), ('a-b', '1'), ('', '1'), ('app', '01'),
                              ('app', 1), ('app', '-1'), ('app', str(2**64))]:
            with self.subTest(name=name, version=version):
                value = package()
                value['package'] = dict(name=name, version=version)
                save(self.manifest, value)
                self.refused('package')

    def test_invalid_json_duplicate_keys_and_nonfinite_numbers(self):
        for raw in [b'{', b'{"schema":1,"schema":1}',
                    b'{"schema":1,"package":{"name":"app","version":"1","version":"2"}}',
                    json.dumps(package(future=float('nan'))).encode(),
                    json.dumps(package(future=float('inf'))).encode(),
                    json.dumps(package(future=1)).replace('"future": 1', '"future": 1e400').encode()]:
            with self.subTest(raw=raw):
                self.manifest.write_bytes(raw)
                self.refused('error:')

    def catalog(self, entries=None, **extra):
        value = package(**extra)
        value.update(schema=2, dependency_catalog=entries if entries is not None else {
            'lib': dict(manifest=str(self.lib), package=dict(name='lib', version='1'))})
        return value

    def test_catalog_preserves_slots_aliases_and_unrelated_data(self):
        ref = dict(dependency='lib', module=['api'], name='Digest')
        value = self.catalog(parameters={
            'H': dict(signature=ref, default={'implementation': ref}),
            'R': dict(signature=ref, default={'parameter': dict(dependency='self', name='H')})},
            future={'unicode': '雪', 'array': [False, None]})
        value['dependency_catalog']['alias'] = copy.deepcopy(value['dependency_catalog']['lib'])
        value['dependencies'] = [dict(use='lib', bindings={
            'H': {'implementation': dict(ref, name='slow')},
            'R': {'parameter': dict(dependency='self', name='H')}},
            expose=[dict(head='provider', module=['api'])])]
        save(self.manifest, value)
        expected = copy.deepcopy(value)
        for entry in expected['dependency_catalog'].values():
            entry['manifest'] = '../library/hirsel.json'
        self.assertEqual(self.success('--write'), expected)
        first = self.manifest.read_bytes()
        self.assertEqual(self.success('--write'), expected)
        self.assertEqual(self.manifest.read_bytes(), first)

    def test_lazy_missing_relative_catalog_is_preserved_even_when_selected(self):
        # The tool cannot infer whether a caller will override this frame's
        # default. Even a direct use is left for compiler selection to validate.
        for selected in (False, True):
            value = self.catalog({'gone': dict(manifest='../absent/./hirsel.json',
                                               package=dict(name='gone', version='1'))})
            if selected:
                value['dependencies'] = [dict(use='gone', bindings={}, expose=[])]
            save(self.manifest, value)
            self.assertEqual(self.success('--write'), value)

    def test_missing_absolute_catalog_is_refused(self):
        save(self.manifest, self.catalog({'gone': dict(manifest=str(self.root / 'absent/hirsel.json'),
                                                      package=dict(name='gone', version='1'))}))
        self.refused('No such file')

    def test_catalog_std_replacement_and_ambiguous_aliases(self):
        std = save(self.root / 'stdlib/hirsel.json', package('std'))
        value = self.catalog({'runtime': dict(manifest='/stale/std.json', package=dict(name='std', version='1'))})
        save(self.manifest, value)
        expected = copy.deepcopy(value)
        expected['dependency_catalog']['runtime']['manifest'] = '../stdlib/hirsel.json'
        self.assertEqual(self.success('--std-manifest', std), expected)
        value['dependency_catalog']['alias'] = copy.deepcopy(value['dependency_catalog']['runtime'])
        save(self.manifest, value)
        self.refused('ambiguous duplicate std@1', '--std-manifest', std)
        self.refused('ambiguous duplicate std@1')

    def test_unused_catalog_paths_are_checked_including_missing_symlink_suffix(self):
        outside = self.root.parent / 'outside'
        outside.mkdir()
        (self.root / 'escape').symlink_to(outside, target_is_directory=True)
        vectors = ['../../missing/hirsel.json', '../escape/absent/hirsel.json',
                   'https://example.com/a', 'file:///tmp/a', '//host/share/a', r'C:\a', r'\\host\share\a']
        for spelling in vectors:
            with self.subTest(spelling=spelling):
                save(self.manifest, self.catalog({'unused': dict(manifest=spelling, package=dict(name='lib', version='1'))}))
                self.refused('error:')
        save(self.manifest, self.catalog({'unused': dict(manifest=str(self.lib), package=dict(name='other', version='1'))}))
        self.refused('identity does not match')

    def test_catalog_malformed_shapes_and_duplicate_keys(self):
        for catalog in ([], None, {'self': {}}, {'a-b': {}}, {'lib': None},
                        {'lib': {'manifest': str(self.lib)}},
                        {'lib': {'manifest': 7, 'package': dict(name='lib', version='1')}}):
            with self.subTest(catalog=catalog):
                value = self.catalog()
                value['dependency_catalog'] = catalog
                save(self.manifest, value)
                self.refused('error:')
        for edges in ([dict(use='absent', bindings={}, expose=[])],
                      [dict(use='lib', bindings=[], expose=[])],
                      [dict(use='lib', bindings={}, expose={})],
                      [dict(use='lib', bindings={}, expose=[])] * 2,
                      [dependency(self.lib)]):
            value = self.catalog()
            value['dependencies'] = edges
            save(self.manifest, value)
            self.refused('error:')
        for raw in ('{"schema":2,"dependency_catalog":{"a":{},"a":{}}}',
                    json.dumps(self.catalog()).replace('"manifest":', '"manifest": "x", "manifest":')):
            self.manifest.write_text(raw)
            self.refused('duplicate JSON field')

    def test_go_catalog_sibling_ancestor_and_dot_vectors(self):
        # Go ebe2feef path_test.go:1495-1578 adapted to manifest targets.
        for spelling, name, expected in [('../library/./hirsel.json', 'lib', '../library/hirsel.json'),
                                          ('../hirsel.json', 'ancestor', '../hirsel.json'),
                                          ('./hirsel.json', 'app', 'hirsel.json')]:
            save(self.root / 'hirsel.json', package('ancestor'))
            value = self.catalog({'edge': dict(manifest=spelling, package=dict(name=name, version='1'))})
            save(self.manifest, value)
            self.assertEqual(self.success()['dependency_catalog']['edge']['manifest'], expected)

    def test_go_relative_path_vectors_on_resolved_existing_paths(self):
        # Go ebe2feefc400 src/path/filepath/path_test.go:1495-1538.
        # Canonicalize first, as the helper does; all directories exist.
        vectors = [('a/b', 'a/b', '.'), ('a/b/.', 'a/b', '.'), ('a/b', 'a/b/.', '.'),
                   ('./a/b', 'a/b', '.'), ('a/b', './a/b', '.'),
                   ('ab/cd', 'ab/cde', '../cde'), ('ab/cd', 'ab/c', '../c'),
                   ('a/b', 'a/b/c/d', 'c/d'), ('a/b', 'a/b/../c', '../c'),
                   ('a/b/../c', 'a/b', '../b'), ('a/b/c', 'a/c/d', '../../c/d'),
                   ('a/b', 'c/d', '../../c/d'), ('a/b/c/d', 'a/b', '../..'),
                   ('a/b/c/d', 'a/b/', '../..'), ('a/b/c/d/', 'a/b', '../..'),
                   ('a/b/c/d/', 'a/b/', '../..'), ('.', 'a/b', 'a/b'), ('.', '..', '..')]
        for base, target, expected in vectors:
            with self.subTest(base=base, target=target):
                paths = [self.root / 'vectors' / p for p in (base, target)]
                for path in paths:
                    path.mkdir(parents=True, exist_ok=True)
                self.assertEqual(API['relative_manifest'](paths[1].resolve(), paths[0].resolve()), expected)

    def test_go_incompatible_root_vector(self):
        # Canonical host paths cannot mix absolute/relative roots. Different
        # Windows volumes still fail in the stdlib relative operation.
        function = API['relative_manifest']
        with patch.dict(function.__globals__, os=SimpleNamespace(path=ntpath)):
            with self.assertRaises(ValueError):
                function(r'D:\lib\hirsel.json', r'C:\app')


class Integration(unittest.TestCase):
    def test_moved_workspace_checks_tests_and_dependency_cards(self):
        self.assertIsNotNone(COMPILER, 'integration requires --hirsel PATH')
        env = os.environ | {'HIRSEL_DAEMON': 'off', 'HIRSEL_RESULTS': 'none@1',
                            'HIRSEL_VALIDATE_STAGES': '1'}
        with tempfile.TemporaryDirectory(prefix='portable-manifest-8-moved-') as temporary:
            original = Path(temporary) / 'original'
            manifest = original / 'app/hirsel.json'
            lib = save(original / 'lib space 雪/hirsel.json', package('portable_lib'))
            (lib.parent / 'main.hir').write_text('pub fn answer() -> u64 uses {} { 42 }\n')
            value = package(dependencies=[dependency(lib, 'portable_lib')],
                            entry=dict(module=[], name='main'))
            save(manifest, value)
            (manifest.parent / 'main.hir').write_text('''pub fn main() -> u64 uses {} { portable_lib@1/answer() }
@test
fn relocated_dependency() -> Result[(), ()] {
  if portable_lib@1/answer() == 42 { Ok(()) } else { Error(()) }
}
@test
fn relocated_relative_source() -> Result[(), ()] {
  if main() == 42 { Ok(()) } else { Error(()) }
}
''')
            result = rewrite(manifest, original, '--write')
            self.assertEqual(result.returncode, 0, result.stderr)
            expected = copy.deepcopy(value)
            expected['dependencies'][0]['manifest'] = '../lib space 雪/hirsel.json'
            self.assertEqual(json.loads(manifest.read_bytes()), expected)
            moved = Path(temporary) / 'moved space 雪'
            shutil.copytree(original, moved)
            shutil.rmtree(original)
            self.assertFalse(original.exists())
            manifest = moved / 'app/hirsel.json'
            self.assertEqual(json.loads(manifest.read_bytes()), expected)
            selected = (manifest.parent / expected['dependencies'][0]['manifest']).resolve(strict=True)
            self.assertTrue(selected.is_relative_to(moved))

            def hirsel(*args):
                result = subprocess.run([str(COMPILER), *map(str, args)], env=env,
                                        cwd=moved, capture_output=True, text=True)
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                self.assertNotIn(str(original), result.stdout + result.stderr)
                return result

            hirsel('check', manifest)
            cards = json.loads(hirsel('deps', 'card', manifest, '--json').stdout)
            self.assertEqual(cards['root_package'], 'app@1')
            self.assertEqual({card['package'] for card in cards['cards']}, {'portable_lib@1'})
            self.assertEqual(cards['cards'][0]['public_functions'], ['portable_lib@1/answer'])
            for engine, args in [('H', []), ('Cranelift', ['--native', '--backend', 'cranelift'])]:
                report = json.loads(hirsel('test', '--json', manifest, *args).stdout)
                self.assertEqual(report['summary'], {'passed': 2, 'failed': 0, 'total': 2})
                self.assertEqual({test['name'].split('/')[-1] for test in report['tests']},
                                 {'relocated_dependency', 'relocated_relative_source'})
                self.assertTrue(all(test['status'] == 'passed' for test in report['tests']))
                print(f'{engine}: 2 selected, 2 passed; original workspace deleted; moved dependency card verified', flush=True)


if __name__ == '__main__':
    if len(sys.argv) == 1:
        sys.argv.append('Unit')
    unittest.main(verbosity=2)
