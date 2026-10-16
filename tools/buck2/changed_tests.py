#!/usr/bin/env python3
"""Run every Buck2 test that this checkout's changes from origin/main can affect.

A test is selected when one of its inputs changed, directly or through any
dependency: its sources, the data it reads, its binaries, the runtime archives
and the build rules that make them (`rdeps(//..., owner(<changed files>))`).
The corpus gate tests (label `gate`) have their own tools/check.sh stages.
"""

import os
from pathlib import Path
import subprocess
import sys
import tomllib

ROOT = Path(__file__).resolve().parents[2]
BUCK2 = ROOT / '.buck2/bin/buck2'
# The configuration tools/check.sh runs Buck2 with, so the daemon keeps one state.
CONFIG = os.environ.get('HIRSEL_BUCK2_CONFIG', '').split()

# A file carrying this marker keeps its own test binary, for example to install
# a global allocator; its first lines say why.
OWN_BINARY = 'own test binary:'


def stray_test_binaries(root):
    """Top-level tests/*.rs files of crates whose integration tests link as tests/it."""
    stray = []
    for main in sorted(Path(root).glob('crates/*/tests/it/main.rs')):
        for path in sorted(main.parent.parent.glob('*.rs')):
            if OWN_BINARY not in path.read_text():
                stray.append(path.relative_to(root).as_posix())
    return stray


# Tests that compile source through the driver live in hirsel-pipeline-tests, so an
# implementation crate's tests are not selected by every edit under the driver.
# Only these crates may depend on the driver,
# or on its engine, which the driver alone wires.
DRIVER = 'hirsel-driver'
ENGINE = 'hirsel-engine'
DRIVER_DEPENDENTS = {
    'hirsel-cli': 'the command-line front end',
    'hirsel-conformance': 'the conformance, agreement and native runners',
    'hirsel-pipeline-tests': 'implementation-crate tests that compile source',
    # These unit-test harnesses read the §22
    # signatures from BuiltinRegistry, which is built with check and modes.
    # hirsel-signature and hirsel-lower have no registry-backed harnesses.
    'hirsel-check': 'body-check harnesses read BuiltinRegistry signatures',
    'hirsel-modes': 'authority and ownership harnesses read BuiltinRegistry signatures',
}


def workspace_metadata(root):
    """The members and their declared dependencies, in `cargo metadata`'s shape."""
    workspace = tomllib.loads((root / 'Cargo.toml').read_text())['workspace']
    manifests = sorted({m for pattern in workspace['members'] for m in root.glob(pattern + '/Cargo.toml')})
    packages = []
    for manifest in manifests:
        data = tomllib.loads(manifest.read_text())
        tables = [(data.get(key, {}), kind) for key, kind in
                  [('dependencies', None), ('dev-dependencies', 'dev'), ('build-dependencies', 'build')]]
        for target in data.get('target', {}).values():
            tables += [(target.get(key, {}), kind) for key, kind in
                       [('dependencies', None), ('dev-dependencies', 'dev'), ('build-dependencies', 'build')]]
        name = data['package']['name']
        packages.append(dict(id=name, name=name, dependencies=[
            dict(name=(spec.get('package', dependency) if isinstance(spec, dict) else dependency), kind=kind)
            for table, kind in tables for dependency, spec in table.items()]))
    return {'workspace_members': [p['id'] for p in packages], 'packages': packages}


def driver_dependents(metadata):
    """Workspace crates outside the allowed set that depend on the driver or the
    engine, of any kind. The driver itself is the engine's one wiring crate."""
    members = set(metadata['workspace_members'])
    return sorted(p['name'] for p in metadata['packages']
                  if p['id'] in members and p['name'] not in DRIVER_DEPENDENTS
                  and any(d['name'] == DRIVER or (d['name'] == ENGINE and p['name'] != DRIVER)
                          for d in p.get('dependencies', [])))


def changed_paths(root):
    # Main may advance while the gate runs. Select this checkout's changes from
    # its common base, rather than treating incoming sibling commits as ours.
    base = subprocess.check_output(['git', 'merge-base', 'HEAD', 'origin/main'],
                                   cwd=root, text=True).strip()
    paths = []
    for arguments in (['git', 'diff', '--no-renames', '--name-only', '-z', base],
                      ['git', 'ls-files', '--others', '--exclude-standard', '-z']):
        paths.extend(os.fsdecode(path) for path in subprocess.check_output(
            arguments, cwd=root).split(b'\0') if path)
    return paths


def owned_paths(root, paths):
    """The files to ask Buck2 about: a deleted file stands for its directory's files."""
    files = set()
    for path in paths:
        if (root / path).is_file():
            files.add(path)
            continue
        directory = (root / path).parent
        while not directory.is_dir():
            directory = directory.parent
        files.update(p.relative_to(root).as_posix() for p in directory.iterdir() if p.is_file())
    return sorted(files)


def query(paths):
    owners = ' + '.join(f'owner("{path}")' for path in paths)
    # owner() sees declared sources, not Starlark loads or workspace settings.
    # A changed build input can affect every test even when no target owns it.
    all_tests = any(path in {'BUCK', 'Cargo.toml', 'Cargo.lock', '.buckconfig', 'rust-toolchain.toml'}
                    or path.startswith(('.cargo/', 'tools/buck2/', 'third-party/')) for path in paths)
    scope = '//...' if all_tests else f'rdeps(//..., {owners})'
    return (f'(kind("_rust_test", {scope}) + attrfilter(labels, doctest, {scope}))'
            ' - attrfilter(labels, gate, //...)')


def run_tests(targets):
    # These tests call full_matrix for toolchain/configuration discovery.
    # Scheduled workspace tests still run them. Keep libtest's --skip away
    # from doctests, whose Buck2 adapter does not accept that option.
    doctests = set(subprocess.check_output(
        [str(BUCK2), 'uquery', *CONFIG,
         'attrfilter(labels, doctest, set(' + ' '.join(targets) + '))'], text=True).split())
    skips = ['native::generated_main_binds_independent_seeds_and_refuses_mismatch_before_entry',
             'native_gate::stored_configuration_retains_independent_seeds_and_baseline_names']
    arguments = [arg for case in skips for arg in ('--test-arg=--skip', '--test-arg=' + case)]
    # Keep all Rust harnesses in one invocation so their long suites overlap.
    for group, extra in (([t for t in targets if t not in doctests], arguments),
                         ([t for t in targets if t in doctests], [])):
        if group:
            status = subprocess.call([str(BUCK2), 'test', *CONFIG, *group,
                                      '--', '--timeout', '1800', *extra])
            if status:
                return status
    return 0


def main():
    os.chdir(ROOT)
    stray = stray_test_binaries(ROOT)
    if stray:
        print('changed tests: each of these links the whole dependency graph as its own '
              'binary; declare it as a module of tests/it/main.rs, or say why it needs its '
              f'own binary with a "// {OWN_BINARY} <reason>" comment: ' + ', '.join(stray),
              file=sys.stderr, flush=True)
        return 1
    dependents = driver_dependents(workspace_metadata(ROOT))
    if dependents:
        print('changed tests: these crates depend on hirsel-driver or hirsel-engine, so every '
              'edit under them would select their tests; move the tests that compile source through '
              'the driver to crates/hirsel-pipeline-tests: ' + ', '.join(dependents),
              file=sys.stderr, flush=True)
        return 1
    paths = owned_paths(ROOT, changed_paths(ROOT))
    if not paths:
        print('changed tests: nothing changed relative to origin/main', flush=True)
        return 0
    targets = subprocess.run([str(BUCK2), 'uquery', *CONFIG, query(paths)], cwd=ROOT, check=True,
                             capture_output=True, text=True).stdout.split()
    if not targets:
        print('changed tests: no test depends on the changed files', flush=True)
        return 0
    print('changed tests: ' + ', '.join(t.removeprefix('root//:') for t in targets), flush=True)
    return run_tests(targets)


if __name__ == '__main__':
    try:
        sys.exit(main())
    except subprocess.CalledProcessError as error:
        print(error.stderr or '', file=sys.stderr)
        sys.exit(error.returncode)
