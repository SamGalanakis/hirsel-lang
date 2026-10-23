#!/usr/bin/env python3
"""Bounded production Ordered runs; independent paths, literals and SHA-256 oracle."""
import argparse
import hashlib
import json
import os
import shutil
from pathlib import Path
import subprocess


def sha(data):
    return hashlib.sha256(data).hexdigest()


def main(args):
    root = args.root
    env = {k: v for k, v in os.environ.items() if not k.startswith('HIRSEL_RT_')}
    env.update(HIRSEL_DAEMON='off', HIRSEL_RESULTS='none@1', HIRSEL_JOBS='4', HIRSEL_MEMORY_LIMIT='2G')
    cases = json.loads((root / 'cases.json').read_text())
    literals = json.loads((root / 'literals.json').read_text())
    if os.environ.get('SITEBUILD_29_CAPS'):
        cases = []
        for vector in json.loads((root / 'caps.json').read_text()):
            cases.append(dict(id=vector['id'], files={f['path']: f['fill']*f['size'] for f in vector['files']},
                exit=2 if vector['error'] else 0, error=vector['error'], hashes=vector['output_hashes']))
    rows = []
    def receipt(id, argv, expected=0, capped=True):
        row = dict(id=id, argv=list(map(str, argv)), state='started',
            source_hashes={p.name:sha(p.read_bytes()) for p in root.glob('*.hir')},
            manifest_sha256=sha((root/'hirsel.json').read_bytes()),
            executable_sha256=sha(Path(argv[0]).read_bytes()),
            helper_sha256=sha(Path(__file__).read_bytes()))
        rows.append(row)
        (root / 'receipts.json').write_text(json.dumps(rows, indent=2))
        command = ['python3', str(root / 'capped'), '--timeout', '60', 'prlimit', '--cpu=60:61', *map(str, argv)] if capped else list(map(str, argv))
        done = subprocess.run(command, env=env, capture_output=True, timeout=300 if not capped else 70)
        (root / (id + '.stdout')).write_bytes(done.stdout)
        (root / (id + '.stderr')).write_bytes(done.stderr)
        row.update(state='completed', exit=done.returncode, stdout=done.stdout.decode(errors='replace'), stderr=done.stderr.decode(errors='replace'), passed=done.returncode == expected)
        (root / 'receipts.json').write_text(json.dumps(rows, indent=2))
        assert done.returncode == expected, (id, done)
        return done
    backends = os.environ.get('SITEBUILD_29_BACKENDS', 'llvm,cranelift').split(',')
    binaries = {}
    for backend in backends:
        binary_dir = Path(os.environ.get('SITEBUILD_29_BINARY_DIR', root))
        binary = binary_dir / backend
        if not os.environ.get('SITEBUILD_29_BINARY_DIR'):
            receipt(backend + '-build', [args.hirsel, 'build', root / 'hirsel.json', '--backend', backend, '-o', binary], capped=False)
        assert binary.is_file()
        binaries[backend] = binary
    provenance = {}
    for name in ['HIRSEL_IMAGE', 'LLVM_SYS_231_PREFIX', 'HIRSEL_RUNTIME_ARCHIVES']:
        value = os.environ.get(name)
        if value:
            path = Path(value).resolve()
            provenance[name] = dict(path=str(path), files={str(p.relative_to(path)) if path.is_dir() else p.name:sha(p.read_bytes()) for p in (sorted(path.rglob('*')) if path.is_dir() else [path]) if p.is_file()})
    (root/'tool-provenance.json').write_text(json.dumps(provenance, indent=2))
    selected = os.environ.get('SITEBUILD_29_CASE')
    if selected:
        cases = [case for case in cases if case['id'] in selected.split(',')]
    assert cases, 'zero selected native cells'
    for backend in backends:
        binary = binaries[backend]
        for width in ([1] if os.environ.get('SITEBUILD_29_CAPS') else [1, 2, 4]):
            for case in cases:
                if width != 1 and case['id'] not in ['nested-three', 'invalid-utf8']:
                    continue
                if case.get('cancel') or case.get('fault'):
                    continue  # Public live file provider cannot inject scope cancellation.
                id = f'{backend}-{width}-{case["id"]}'
                cell = root / id
                data = cell / 'data'
                (data / 'content').mkdir(parents=True)
                if not case.get('missing_assets'):
                    (data / 'assets').mkdir()
                for name, content in case['files'].items():
                    target = data / name
                    target.parent.mkdir(parents=True, exist_ok=True)
                    target.write_bytes(content.encode() if isinstance(content, str) else bytes(content))
                for directory in case.get('directories', []):
                    (data / directory).mkdir(parents=True, exist_ok=True)
                cfg = f'content = "content"\nassets = "assets"\noutput = "{case.get("output", "site")}"\ntitle = "Site & sheep"\njobs = {case.get("jobs", 4)}\n'
                (data / 'site.toml').write_text(cfg)
                argv = [] if case.get('legacy') else [case.get('command', 'build'), 'site.toml']
                grants = dict(args=argv, io=dict(stdout='stdout.bytes@1', tasks='tasks.cooperative@1', dirs=[dict(path='data', host=str(data), access='read' if case.get('read_only') else 'read_write')]), placement=dict(cores=width, commit='shared', reorder='off'))
                if case.get('legacy'):
                    grants['io'].pop('tasks')
                grant = cell / 'grants.json'
                grant.write_text(json.dumps(grants))
                tape = cell / 'record.tape'
                done = receipt(id, [binary, '--hirsel-record', tape, '--hirsel-grants', grant, *argv], case['exit'])
                if 'stdout' in case:
                    assert done.stdout.decode() == case['stdout'], (id, done)
                elif case.get('error'):
                    assert case['error'] in done.stdout.decode(), (id, done)
                if case.get('legacy'):
                    assert {p.relative_to(data/'site').as_posix():p.read_text() for p in (data/'site').rglob('*') if p.is_file()} == case['outputs']
                if case['exit'] == 0 and case.get('command') != 'plan' and not case.get('legacy'):
                    manifest = json.loads((data / 'site/.mdsite.json').read_text())
                    assert manifest['config'] == sha(cfg.encode())
                    assert set(p.relative_to(data / 'site').as_posix() for p in (data / 'site').rglob('*') if p.is_file()) == set(manifest['files']) | {'.mdsite.json'}
                    for name, entry in manifest['files'].items():
                        assert sha((data / 'site' / name).read_bytes()) == entry['output']
                    for name, expected in case.get('outputs', {}).items():
                        assert (data / 'site' / name).read_bytes() == expected.encode(), (id, name)
                    for name, expected in case.get('hashes', {}).items():
                        assert sha((data / 'site' / name).read_bytes()) == expected, (id, name)
                    if case['id'] == 'nested-three':
                        assert literals['a'].encode() in (data / 'site/content/a.md.html').read_bytes()
                        assert literals['b'].encode() in (data / 'site/content/sub/b.md.html').read_bytes()
                        assert b'content/a%20%26%20b.md.html' in (data / 'site/index.html').read_bytes()
                    before = {p: (p.read_bytes(), p.stat().st_ino) for p in (data / 'site').rglob('*') if p.is_file() and p.name != '.mdsite.json'}
                    again = receipt(id + '-unchanged', [binary, '--hirsel-grants', grant, *argv])
                    assert b'write 0;' in again.stdout
                    assert all(p.read_bytes() == value[0] and p.stat().st_ino == value[1] for p, value in before.items())
                if width == 1 and case['id'] == 'nested-three':
                    target = data / 'site/content/a.md.html'
                    target.write_bytes(b'corrupt')
                    repaired = receipt(id + '-corrupt-output', [binary, '--hirsel-grants', grant, *argv])
                    assert repaired.stdout == b'pages 3; write 1; skip 4\n'
                    assert target.read_bytes() == case['outputs']['content/a.md.html'].encode()
                    (data / 'content/a.md').write_text('# Changed')
                    changed = receipt(id + '-modified', [binary, '--hirsel-grants', grant, *argv])
                    assert changed.stdout == b'pages 3; write 4; skip 1\n'
                    assert b'<h1>Changed</h1>' in target.read_bytes()
                    assert b'>Changed</a>' in (data / 'site/content/sub/b.md.html').read_bytes()
                    unrelated = data / 'site/handwritten.txt'
                    unrelated.write_text('leave me')
                    (data / 'content/a.md').unlink()
                    deleted = receipt(id + '-deleted', [binary, '--hirsel-grants', grant, *argv])
                    assert deleted.stdout == b'pages 2; write 3; skip 1\nstale content/a.md.html\n'
                    assert target.is_file() and unrelated.read_text() == 'leave me'
                    manifest = json.loads((data / 'site/.mdsite.json').read_text())
                    assert 'content/a.md.html' not in manifest['files']
                    (data / 'site.toml').write_text(cfg.replace('Site & sheep', 'Changed site'))
                    reconfigured = receipt(id + '-changed-config', [binary, '--hirsel-grants', grant, *argv])
                    assert reconfigured.stdout == b'pages 2; write 3; skip 1\n'
                    assert b'Changed site' in (data / 'site/index.html').read_bytes()
                # Original providers become unavailable for every immutable replay.
                tape_hash = sha(tape.read_bytes())
                data.rename(cell / 'unavailable')
                frozen_tree = {p.relative_to(cell/'unavailable').as_posix():sha(p.read_bytes()) for p in (cell/'unavailable').rglob('*') if p.is_file()}
                for replay_engine in ['reference', *backends]:
                    if os.environ.get('SITEBUILD_29_CAPS') and replay_engine == 'reference' and case['id'] in ['items-64', 'page-65536', 'input-1048576', 'output-2097152', 'output-2097153']:
                        rows.append(dict(id=id+'-replay-reference', state='unrun', passed=False, disposition='INCOMPLETE: large boundary reference replay not attempted after public adapter 1M limit'))
                        continue
                    command = [args.hirsel, 'run', root / 'hirsel.json', '--replay', tape] if replay_engine == 'reference' else [binaries[replay_engine], '--hirsel-replay', tape]
                    replay = receipt(id + '-replay-' + replay_engine, command, case['exit'])
                    if replay.returncode == case['exit']:
                        assert replay.stdout == done.stdout
                    assert sha(tape.read_bytes()) == tape_hash
                    assert not data.exists()
                    assert frozen_tree == {p.relative_to(cell/'unavailable').as_posix():sha(p.read_bytes()) for p in (cell/'unavailable').rglob('*') if p.is_file()}
    # The replay parser must reject a corrupt immutable-input copy.
    good = root / 'llvm-1-empty/record.tape'
    if good.is_file():
        bad = root / 'corrupt.tape'
        bad.write_bytes(good.read_bytes() + b'changed-tape\n')
        receipt('changed-tape-refused', [args.hirsel, 'run', root / 'hirsel.json', '--replay', bad], expected=3)
    print(json.dumps(dict(selected=len(rows), completed=sum(row['state']=='completed' for row in rows), passed=sum(row['passed'] for row in rows), incomplete=sum(row.get('disposition', '').startswith('INCOMPLETE') for row in rows))))


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--hirsel', type=Path, required=True)
    parser.add_argument('--root', type=Path, required=True)
    main(parser.parse_args())
