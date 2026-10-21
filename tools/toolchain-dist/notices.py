#!/usr/bin/env python3
"""Collect release notices from the configured link graph and pinned crate archives.

No SPDX-to-text substitution: retain the actual upstream texts, including nested
C/assembly notices. Paths in the result are relative and timestamps are absent.
"""
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import re
import shutil
import subprocess
import tarfile
import tempfile
import tomllib

ROOT = Path(__file__).resolve().parents[2]
SOURCES = Path(__file__).with_name('notice-sources.json')
LICENSE_NAME = re.compile(r'^(licen[cs]e|copying|copyright|notice)([._-].*)?$', re.I)


def sha256(path):
    with path.open('rb') as handle:
        return hashlib.file_digest(handle, 'sha256').hexdigest()


def linked_crates(graph, root):
    """Follow link edges, not build scripts, proc macros or implicit toolchains.

    Archive source edges identify packages without guessing Reindeer's aliases.
    The inventory is of link inputs, before linker dead-code elimination.
    """
    seen, crates = set(), {}

    def visit(label):
        if label in seen:
            return
        seen.add(label)
        attrs = graph[label]
        if attrs.get('proc_macro'):
            return
        for dependency in attrs.get('buck.deps', []):
            match = re.match(r'root//third-party:([^ :]+)\.crate \(', dependency)
            if match:
                crates.setdefault(match[1], set()).add(label.split(' (')[0])
        edges = attrs.get('deps', []) + attrs.get('exported_deps', [])
        edges += list(attrs.get('named_deps', {}).values())
        edges += [edge[0] for edge in attrs.get('flagged_deps', [])]
        if not any(key in attrs for key in ('deps', 'named_deps', 'proc_macro')):
            # Alias rules have only implicit buck.deps; stop at toolchain cells.
            edges = attrs.get('buck.deps', [])
        for dependency in edges:
            if dependency.startswith('root//'):
                visit(dependency)

    matches = [label for label in graph if label.startswith(root + ' (')]
    if len(matches) != 1:
        raise ValueError(f'expected one configured root {root}, found {len(matches)}')
    visit(matches[0])
    if not crates:
        raise ValueError('empty linked crate inventory')
    return {name: sorted(targets) for name, targets in sorted(crates.items())}


def crate_texts(archive, checksum, package, required=(), allow_empty=False):
    if sha256(archive) != checksum:
        raise ValueError(f'{package}: crate archive checksum mismatch')
    texts = {}
    primary = False
    prefix = package['name'] + '-' + package['version'] + '/'
    declared = package.get('license_file')
    if declared:
        if Path(declared).is_absolute():
            declared = os.path.relpath(declared, Path(package['manifest_path']).parent)
    with tarfile.open(archive) as source:
        for member in source:
            if not member.name.startswith(prefix) or not member.isfile():
                continue
            relative = member.name[len(prefix):]
            path = PurePosixPath(relative)
            if path.is_absolute() or '..' in path.parts:
                raise ValueError('unsafe crate path: ' + relative)
            data = source.extractfile(member).read()
            named = LICENSE_NAME.fullmatch(path.name) or relative == declared
            if named:
                if not data.strip():
                    raise ValueError('empty license: ' + relative)
                primary = True
            # Preserve full upstream files with embedded notices: extracting a
            # guessed comment prefix can truncate a copyright or permission grant.
            embedded = b'\0' not in data and re.search(rb'copyright|SPDX-License-Identifier', data, re.I)
            if named or embedded:
                texts[relative] = data
    missing = set(required) - texts.keys()
    if declared and declared not in texts:
        missing.add(declared)
    if missing or (not primary and not allow_empty):
        raise ValueError(f"{prefix}missing license text: {sorted(missing) or 'no upstream license files'}")
    return texts


def pinned_archive(cargo_home, name, checksum):
    archives = sorted((cargo_home / 'registry/cache').glob('*/' + name + '.crate'))
    for archive in archives:
        if sha256(archive) == checksum:
            return archive
    if archives:
        raise ValueError(f'{name}: crate archive checksum mismatch')
    raise ValueError(f'{name}: pinned crate archive unavailable in Cargo cache')


def omitted_text(archive, package, fallback, cargo_home, locked):
    """Explicit, reviewed per-version upstream omission; never an SPDX fallback."""
    donor = fallback['donor']
    donor_archive = pinned_archive(cargo_home, donor, locked[donor]['checksum'])
    for path, prefix in ((archive, package), (donor_archive, donor)):
        with tarfile.open(path) as source:
            vcs = json.load(source.extractfile(prefix + '/.cargo_vcs_info.json'))
        if vcs['git']['sha1'] != fallback['revision']:
            raise ValueError(f'{package}: fallback revision mismatch')
    with tarfile.open(donor_archive) as source:
        data = source.extractfile(donor + '/' + fallback['path']).read()
    if hashlib.sha256(data).hexdigest() != fallback['sha256']:
        raise ValueError(f'{package}: fallback license checksum mismatch')
    return {fallback['path']: data}


def file_inventory(bundle):
    return {p.relative_to(bundle).as_posix(): sha256(p)
            for p in sorted(bundle.rglob('*'))
            if p.is_file() and 'notices' not in p.relative_to(bundle).parts
            and p.name != 'hirsel-build.json'}


def generate(bundle, graph, metadata, root=ROOT, cargo_home=None, sources=None):
    """Only add notices; refuse missing inputs without leaving a partial directory."""
    sources = sources or json.loads(SOURCES.read_text())
    cargo_home = cargo_home or Path(os.environ.get('CARGO_HOME', Path.home() / '.cargo'))
    inventory = linked_crates(graph, sources['root'])
    packages = {p['name'] + '-' + p['version']: p for p in metadata['packages'] if p['source']}
    lock = tomllib.loads((root / 'Cargo.lock').read_text())
    locked = {p['name'] + '-' + p['version']: p for p in lock['package'] if 'source' in p}
    allowed = set(sources['bundle_entries']) | {'notices', 'hirsel-build.json', 'hirsel.profdata'}
    unexpected = {p.name for p in bundle.iterdir()} - allowed
    if unexpected:
        raise ValueError(f'uninventoried bundle entries: {sorted(unexpected)}')
    missing = set(sources['bundle_entries']) - {p.name for p in bundle.iterdir()}
    if missing:
        raise ValueError(f'missing bundle entries: {sorted(missing)}')
    before = file_inventory(bundle)
    if not before or 'hirsel' not in before:
        raise ValueError('bundle has no compiler')
    destination = bundle / 'notices'
    if destination.exists():
        raise ValueError('notices already exists; use a fresh bundle')
    manifest = dict(schema=1, scope=sources['scope'], components=[],
                    external=sources['external'], files=before,
                    inputs={'Cargo.lock': sha256(root / 'Cargo.lock')})
    with tempfile.TemporaryDirectory(prefix='.notices-', dir=bundle.parent) as raw:
        stage = Path(raw)

        def retain(name, data):
            path = stage / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(data)
            return dict(path=name, sha256=hashlib.sha256(data).hexdigest())

        for name, targets in inventory.items():
            package, pin = packages[name], locked[name]
            if package['source'] != pin['source'] or not pin['source'].startswith('registry+'):
                raise ValueError(f'{name}: unsupported or mismatched package source')
            archive = pinned_archive(cargo_home, name, pin['checksum'])
            fallback = sources.get('archive_omissions', {}).get(name)
            texts = crate_texts(archive, pin['checksum'], package,
                                sources['required_texts'].get(name, []), allow_empty=bool(fallback))
            used_fallback = bool(fallback) and not any(LICENSE_NAME.fullmatch(PurePosixPath(path).name) for path in texts)
            if used_fallback:
                texts.update(omitted_text(archive, name, fallback, cargo_home, locked))
            record = dict(name=package['name'], version=package['version'],
                          source=pin['source'], archive_sha256=pin['checksum'],
                          url=f"https://static.crates.io/crates/{package['name']}/{package['version']}/download",
                          declared_license=package['license'], targets=targets,
                          selection='All upstream license/notice files and full text files with embedded copyright/SPDX notices retained; no alternative license elected.',
                          texts=[dict(upstream=path, **retain(f'crates/{name}/{path}', data))
                                 for path, data in sorted(texts.items())])
            if used_fallback:
                record['archive_omission'] = dict(fallback, donor_archive_sha256=locked[fallback['donor']]['checksum'])
            if not package['license']:
                record['gap'] = 'No Cargo license expression; inspect upstream license files.'
            manifest['components'].append(record)

        rust_lock = json.loads((root / sources['rust_lock']).read_text())
        rust = root / sources['rust_tree']
        if json.loads((rust / '.hirsel-toolchain.json').read_text()) != rust_lock:
            raise ValueError('Rust license inputs do not match the pinned toolchain')
        docs = rust / 'share/doc/rust'
        copyright_file = docs / 'COPYRIGHT-library.html'
        if not copyright_file.is_file() or not (docs / 'licenses').is_dir():
            raise ValueError('pinned Rust standard-library license inputs unavailable')
        rust_texts = [copyright_file, *sorted((docs / 'licenses').glob('*'))]
        if not (docs / 'licenses/MIT.txt').is_file() or not (docs / 'licenses/Apache-2.0.txt').is_file():
            raise ValueError('missing Rust license text')
        manifest['components'].append(dict(
            name='Rust standard library', version=rust_lock['rust_version'],
            source=rust_lock, selection='Upstream library copyright inventory and complete accompanying license directory.',
            texts=[dict(upstream=p.relative_to(rust).as_posix(),
                        **retain('rust/' + p.relative_to(docs).as_posix(), p.read_bytes())) for p in rust_texts]))
        for name in sources['adapted_source_notices']:
            retain('adapted-source/' + name, (root / name).read_bytes())
        retain('HIRSEL-LICENSE', (root / 'LICENSE').read_bytes())
        for name in [sources['rust_lock'], 'tools/toolchain-dist/notice-sources.json',
                     'tools/toolchain-dist/notices.py', 'Cargo.toml',
                     *[pin for component in sources['external'] for pin in component.get('pins', [])], *sorted(str(p.relative_to(root)) for p in (root / 'crates').glob('*/Cargo.toml')),
                     *sorted(str(p.relative_to(root)) for p in (root / 'third-party/fixups').rglob('*') if p.is_file())]:
            manifest['inputs'][name] = sha256(root / name)
        manifest['notice_files'] = {p.relative_to(stage).as_posix(): sha256(p) for p in sorted(stage.rglob('*')) if p.is_file()}
        (stage / 'manifest.json').write_text(json.dumps(manifest, indent=2, sort_keys=True) + '\n')
        if file_inventory(bundle) != before:
            raise ValueError('bundle changed while collecting notices')
        shutil.copytree(stage, destination)
    return manifest


def collect(bundle, mode):
    configuration = ['--target-platforms', '//tools/buck2:dist', '-c', 'hirsel.dist_pgo=' + mode]
    graph = json.loads(subprocess.check_output([
        str(ROOT / '.buck2/bin/buck2'), 'cquery', *configuration,
        'deps(root//:hirsel-cli-bin-hirsel)',
        '--output-attribute', r'^(buck\.deps|deps|exported_deps|named_deps|flagged_deps|proc_macro)$', '--json'], cwd=ROOT))
    metadata = json.loads(subprocess.check_output([
        os.environ.get('CARGO', 'cargo'), 'metadata', '--locked', '--format-version', '1',
        '--filter-platform', 'x86_64-unknown-linux-gnu'], cwd=ROOT))
    return generate(bundle, graph, metadata)
