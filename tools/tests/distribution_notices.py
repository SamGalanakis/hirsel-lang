#!/usr/bin/env python3
"""Packaging controls use a pinned archive and a tiny relocatable bundle."""
import importlib.util
import io
import json
import os
from pathlib import Path
import shutil
import tarfile
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location('notices', Path(os.environ.get('HIRSEL_DISTRIBUTION_NOTICES', ROOT / 'tools/toolchain-dist/notices.py')))
notices = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(notices)


def archive(path, files, prefix='example-1.0'):
    with tarfile.open(path, 'w:gz') as output:
        for name, data in files.items():
            member = tarfile.TarInfo(prefix + '/' + name)
            member.size = len(data)
            output.addfile(member, io.BytesIO(data))


class Notices(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.bundle = self.root / 'bundle'
        self.bundle.mkdir()
        (self.bundle / 'hirsel').write_bytes(b'compiler object bytes')
        (self.bundle / 'hirsel-std.objects').mkdir()
        (self.bundle / 'hirsel-std.objects/std.o').write_bytes(b'std object bytes')
        self.home = self.root / 'cargo'
        self.crate = self.home / 'registry/cache/test/example-1.0.crate'
        self.crate.parent.mkdir(parents=True)
        archive(self.crate, {'LICENSE-MIT': b'actual copyright and MIT terms',
                             'native/LICENSE': b'actual native terms',
                             'NOTICE': b'upstream notice',
                             'native/code.c': b'/* Copyright Author; permission notice */ int code;'})
        self.package = dict(name='example', version='1.0', source='registry+test',
                            license='MIT OR Apache-2.0', license_file=None,
                            manifest_path='/irrelevant/cache/example-1.0/Cargo.toml')
        self.metadata = dict(packages=[self.package])
        self.write_lock()
        self.sources = dict(root='root//:compiler', scope='fixture', bundle_entries=['hirsel', 'hirsel-std.objects'],
                            rust_lock='rust-pin.json', rust_tree='rust',
                            required_texts={}, adapted_source_notices=['third-party/NOTICES.md'], external=[])
        self.graph = {
            'root//:compiler (dist#1)': {'deps': ['root//third-party:alias (dist#1)', 'root//:macro (host#2)']},
            'root//third-party:alias (dist#1)': {'buck.deps': ['root//third-party:library (dist#1)']},
            'root//third-party:library (dist#1)': {'deps': [], 'buck.deps': ['root//third-party:example-1.0.crate (dist#1)']},
            'root//:macro (host#2)': {'proc_macro': True, 'deps': ['absent macro-only crate']},
        }
        for name in ['LICENSE', 'Cargo.toml', 'third-party/NOTICES.md', 'tools/toolchain-dist/notice-sources.json', 'tools/toolchain-dist/notices.py']:
            target = self.root / name
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_text('fixture source input')
        (self.root / 'rust-pin.json').write_text('{"rust_version":"test"}')
        rust = self.root / 'rust'
        (rust / 'share/doc/rust/licenses').mkdir(parents=True)
        (rust / '.hirsel-toolchain.json').write_text((self.root / 'rust-pin.json').read_text())
        for name in ['COPYRIGHT-library.html', 'licenses/MIT.txt', 'licenses/Apache-2.0.txt']:
            (rust / 'share/doc/rust' / name).write_text('actual upstream ' + name)

    def write_lock(self):
        (self.root / 'Cargo.lock').write_text(
            '[[package]]\nname = "example"\nversion = "1.0"\nsource = "registry+test"\n'
            f'checksum = "{notices.sha256(self.crate)}"\n')

    def generate(self, bundle=None):
        return notices.generate(bundle or self.bundle, self.graph, self.metadata,
                                self.root, self.home, self.sources)

    def test_inventory_uses_link_edges_and_excludes_proc_macro(self):
        self.assertEqual(notices.linked_crates(self.graph, self.sources['root']),
                         {'example-1.0': ['root//third-party:library']})

    def test_determinism_and_relocation_preserve_objects_and_native_texts(self):
        before = notices.file_inventory(self.bundle)
        copy = self.root / 'different location'
        shutil.copytree(self.bundle, copy)
        first = self.generate()
        self.generate(copy)
        self.assertEqual((copy / 'notices/manifest.json').read_bytes(),
                         (self.bundle / 'notices/manifest.json').read_bytes())
        self.assertEqual(notices.file_inventory(copy), before)
        self.assertEqual(first['files'], before)
        self.assertEqual(len(first['components'][0]['texts']), 4)
        shutil.move(copy, self.root / 'relocated')
        self.assertEqual(notices.file_inventory(self.root / 'relocated'), before)
        self.assertTrue((self.root / 'relocated/notices/crates/example-1.0/native/LICENSE').is_file())
        self.assertEqual((copy.parent / 'relocated/notices/crates/example-1.0/native/code.c').read_bytes(),
                         b'/* Copyright Author; permission notice */ int code;')
        self.assertNotIn(str(self.root), json.dumps(first))

    def test_missing_license_refuses_without_partial_output(self):
        archive(self.crate, {'src/lib.rs': b'code'})
        self.write_lock()
        with self.assertRaisesRegex(ValueError, 'missing license text'):
            self.generate()
        self.assertFalse((self.bundle / 'notices').exists())

    def test_missing_declared_license_file_refuses_even_with_other_texts(self):
        self.package['license_file'] = '/irrelevant/cache/example-1.0/LEGAL.txt'
        with self.assertRaisesRegex(ValueError, 'LEGAL.txt'):
            self.generate()

    def test_declared_license_file_with_unusual_name_is_retained(self):
        self.package['license_file'] = '/irrelevant/cache/example-1.0/LEGAL.txt'
        archive(self.crate, {'LEGAL.txt': b'actual custom terms'})
        self.write_lock()
        self.generate()
        self.assertEqual((self.bundle / 'notices/crates/example-1.0/LEGAL.txt').read_bytes(), b'actual custom terms')

    def test_tampered_archive_refuses(self):
        self.crate.write_bytes(b'corrupt')
        with self.assertRaisesRegex(ValueError, 'checksum mismatch'):
            self.generate()

    def test_missing_archive_refuses(self):
        self.crate.unlink()
        with self.assertRaisesRegex(ValueError, 'archive unavailable'):
            self.generate()

    def test_missing_required_native_text_refuses(self):
        self.sources['required_texts'] = {'example-1.0': ['native/NOTICE']}
        with self.assertRaisesRegex(ValueError, 'native/NOTICE'):
            self.generate()

    def test_missing_rust_text_refuses(self):
        (self.root / 'rust/share/doc/rust/licenses/MIT.txt').unlink()
        with self.assertRaisesRegex(ValueError, 'missing Rust license text'):
            self.generate()
        self.assertFalse((self.bundle / 'notices').exists())

    def test_uninventoried_native_bundle_refuses(self):
        (self.bundle / 'libLLVM.so').write_bytes(b'extra code')
        with self.assertRaisesRegex(ValueError, 'uninventoried bundle entries'):
            self.generate()

    def fallback_fixture(self):
        data = b'actual same-revision upstream terms'
        import hashlib
        archive(self.crate, {'.cargo_vcs_info.json': b'{"git":{"sha1":"revision"}}'})
        self.write_lock()
        donor = self.crate.with_name('donor-2.0.crate')
        archive(donor, {'.cargo_vcs_info.json': b'{"git":{"sha1":"revision"}}', 'LICENSE': data}, 'donor-2.0')
        with (self.root / 'Cargo.lock').open('a') as lock:
            lock.write('[[package]]\nname = "donor"\nversion = "2.0"\nsource = "registry+test"\n'
                       f'checksum = "{notices.sha256(donor)}"\n')
        fallback = dict(donor='donor-2.0', path='LICENSE', revision='revision',
                        sha256=hashlib.sha256(data).hexdigest(), upstream='upstream source', reason='archive omission')
        self.sources['archive_omissions'] = {'example-1.0': fallback}
        return fallback

    def test_verified_same_revision_fallback_records_omission(self):
        self.fallback_fixture()
        record = self.generate()['components'][0]
        self.assertEqual(record['archive_omission']['donor'], 'donor-2.0')
        self.assertEqual((self.bundle / 'notices/crates/example-1.0/LICENSE').read_bytes(),
                         b'actual same-revision upstream terms')

    def test_fallback_refuses_another_revision(self):
        self.fallback_fixture()['revision'] = 'wrong revision'
        with self.assertRaisesRegex(ValueError, 'revision mismatch'):
            self.generate()

    def test_fallback_refuses_another_text(self):
        self.fallback_fixture()['sha256'] = 'wrong digest'
        with self.assertRaisesRegex(ValueError, 'license checksum mismatch'):
            self.generate()

    def test_missing_expression_is_reported_without_electing_a_license(self):
        self.package['license'] = None
        record = self.generate()['components'][0]
        self.assertIn('gap', record)
        self.assertIn('no alternative license elected', record['selection'])


if __name__ == '__main__':
    unittest.main()
