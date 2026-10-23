"""Go path.Match vectors (ebe2feef, BSD-3-Clause, The Go Authors), hashlib oracle.

See examples/filemanifest/README.md for upstream mapping and adaptations.
No Python glob is used. Every runtime child is bounded, outputs retained separately
from tools/capped's resource report, and private fixture cleanup is asserted.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys


def canonical(value):
    return json.dumps(value, ensure_ascii=False, separators=(',', ':')).encode() + b'\n'


def entry(name, data):
    return dict(path=name, size=len(data), sha256=hashlib.sha256(data).hexdigest())


def manifest(entries):
    return dict(schema=1, entries=sorted(entries, key=lambda row: row['path'].encode()))


def error(kind, path=''):
    return 2, b'', canonical(dict(**{'class': 'invalid_input'}, kind=kind, path=path))


def snapshot(root):
    result = {}
    for path in sorted(root.rglob('*')):
        rel = str(path.relative_to(root))
        if path.is_symlink():
            result[rel] = ('link', os.readlink(path))
        elif path.is_file():
            result[rel] = ('file', path.stat().st_mode, hashlib.sha256(path.read_bytes()).hexdigest())
        else:
            result[rel] = ('directory', path.stat().st_mode)
    return result


def verify(hirsel, root, output):
    output.mkdir(parents=True, exist_ok=False)
    selection = root / 'hirsel.json'
    commands = []
    # The wrapper records target streams untouched; capped's diagnostics have their
    # own file. RLIMIT_CPU applies to each runtime, independently of setup builds.
    child = output / 'child.py'
    child.write_text('''import pathlib,resource,subprocess,sys
resource.setrlimit(resource.RLIMIT_CPU,(60,60))
p=subprocess.run(sys.argv[3:],stdin=subprocess.DEVNULL,capture_output=True)
pathlib.Path(sys.argv[1]).write_bytes(p.stdout)
pathlib.Path(sys.argv[2]).write_bytes(p.stderr)
sys.exit(p.returncode)
''')
    env = os.environ | dict(HIRSEL_DAEMON='off', HIRSEL_RESULTS='none@1',
                            HIRSEL_JOBS='1', HIRSEL_MEMORY_LIMIT='2G', HIRSEL_TIMING='off')

    def run(label, argv, runtime=False):
        argv = list(map(str, argv))
        stdout = output / (label + '.stdout')
        stderr = output / (label + '.stderr')
        command = ([sys.executable, root / 'capped', '--timeout', '60',
                    sys.executable, child, stdout, stderr, *argv] if runtime else argv)
        done = subprocess.run(list(map(str, command)), stdin=subprocess.DEVNULL,
                              capture_output=True, env=env, timeout=120 if runtime else 300)
        if runtime:
            (output / (label + '.resources')).write_bytes(done.stderr)
            assert stdout.exists() and stderr.exists(), (label, done.returncode, done.stderr)
            result = done.returncode, stdout.read_bytes(), stderr.read_bytes()
        else:
            stdout.write_bytes(done.stdout); stderr.write_bytes(done.stderr)
            result = done.returncode, done.stdout, done.stderr
        commands.append(dict(label=label, argv=argv, exit=done.returncode))
        (output / 'commands.json').write_text(json.dumps(commands, indent=2)+'\n')
        return result

    assert run('fmt', [hirsel, 'fmt', '--check', selection]) == (0, b'', b'')
    assert run('check', [hirsel, 'check', '--json', selection]) == (0, b'', b'')
    assert run('export', [hirsel, 'export', '--tir', selection, '-o', output/'app.tir']) == (0, b'', b'')
    assert (output/'app.tir').stat().st_size > 0
    binaries = {}
    for backend in ('llvm', 'cranelift'):
        exe = output / backend
        result = run('build-'+backend, [hirsel, 'build', selection, '--backend', backend, '-o', exe])
        assert result == (0, b'', b''), result
        binaries[backend] = exe

    data = output / 'data'; data.mkdir()
    outside = output / 'outside'; outside.mkdir()
    (outside/'secret').write_bytes(b'outside grant: unchanged\n')
    cases = []

    def case(label, files, args, expected, links=None, grant=None, fixture=False):
        directory = data / label
        directory.mkdir()
        if fixture:
            shutil.copytree(root/'fixtures/release', directory, dirs_exist_ok=True)
        for name, raw in files.items():
            p = directory/name; p.parent.mkdir(parents=True, exist_ok=True); p.write_bytes(raw)
        for name, target in (links or {}).items():
            (directory/name).symlink_to(target)
        cases.append((label, directory, args, expected, grant))

    good = dict(schema=1, missing=[], changed=[])
    release = [('abc.txt', b'abc'), ('docs/notes.txt', b'Ready for review.\n'), ('empty.txt', b'')]
    # All 30 exact published Match rows in match_test.go:19-49 are checked via the
    # application's selection, including scalar ?, classes and escaped '*'.
    vectors = json.loads((root/'fixtures/go-match.json').read_text())
    for i, row in enumerate(vectors):
        expected = manifest([entry(row['path'], b'abc')] if row['matches'] else [])
        case(f'go-{i:02}', {row['path']: b'abc'}, ['scan', 'fixtures', row['pattern']],
             (0, canonical(expected), b''))
    case('a-slash-b', {'a/b': b''}, ['scan', 'fixtures', 'a*'], (0, canonical(manifest([])), b''))
    case('realistic-top', {}, ['scan', 'fixtures', '*.txt'],
         (0, canonical(manifest([entry(n,d) for n,d in release if '/' not in n])), b''), fixture=True)
    case('nested', {}, ['scan', 'fixtures', 'docs/*.txt'],
         (0, canonical(manifest([entry(*release[1])])), b''), fixture=True)
    case('realistic-verify', {}, ['verify', 'fixtures', 'selected.json'],
         (0, canonical(good), b''), fixture=True)
    case('realistic-invalid', {}, ['verify', 'fixtures', 'invalid.json'],
         error('invalid_path', '../secrets/token'), fixture=True)
    expected = manifest([entry(n,d) for n,d in release])
    case('changed-missing', {'abc.txt':b'ABC','empty.txt':b'', 'selected.json':canonical(expected)},
         ['verify','fixtures','selected.json'],
         (1, canonical(dict(schema=1, missing=['docs/notes.txt'], changed=['abc.txt'])), b''))
    case('empty', {}, ['scan','fixtures','*'], (0,canonical(manifest([])),b''))
    case('order', {'z':b'z','A':b'A','é':b'\xff','a':b'a','a space':b'\0'}, ['scan','fixtures','*'],
         (0, canonical(manifest([entry(n,d) for n,d in [('A',b'A'),('a',b'a'),('a space',b'\0'),('z',b'z'),('é',b'\xff')]])), b''))
    case('help', {}, ['--help'], (0, None, b''), grant='dir')
    case('usage', {}, [], error('usage'))
    case('missing-grant', {}, ['scan','fixtures','*'], error('directory_grant','fixtures'), grant='dir')
    case('denied-root', {}, ['scan','outside','*'], error('directory_grant','outside'))
    case('bad-pattern', {}, ['scan','fixtures','['], error('pattern_syntax'))
    case('pattern-cap', {}, ['scan','fixtures','a'*129], error('pattern_bytes_limit'))
    for label, raw, problem in [('bad-json',b'{','manifest_json'),('bad-utf8',b'\xff','manifest_utf8'),
                                ('oversize',b' '*65537,'input_bytes_limit'),
                                ('duplicate-key',b'{"schema":1,"schema":1,"entries":[]}', 'manifest_json')]:
        case(label, {'manifest':raw}, ['verify','fixtures','manifest'], error(problem,'manifest' if label=='oversize' else ''))
    blank=canonical(manifest([]))
    case('input-exact', {'manifest':blank+b' '*(65536-len(blank))}, ['verify','fixtures','manifest'], (0,canonical(good),b''))
    case('file-exact', {'a':b'x'*4096}, ['scan','fixtures','*'], (0,canonical(manifest([entry('a',b'x'*4096)])),b''))
    case('file-over', {'a':b'x'*4097}, ['scan','fixtures','*'], error('file_bytes_limit','a'))
    exact={str(i):b'x'*4096 for i in range(4)}
    case('total-exact', exact, ['scan','fixtures','*'], (0,canonical(manifest([entry(n,d) for n,d in exact.items()])),b''))
    case('total-over', dict(exact, z=b'x'), ['scan','fixtures','*'], error('total_bytes_limit','z'))
    many={f'{i:03}':b'' for i in range(64)}
    case('records-exact', many, ['scan','fixtures','*'], (0,canonical(manifest([entry(n,d) for n,d in many.items()])),b''))
    case('records-over', dict(many, z=b''), ['scan','fixtures','*'], error('record_limit'))
    case('visited-exact', {str(i):b'' for i in range(128)}, ['scan','fixtures','no-match'], (0,canonical(manifest([])),b''))
    case('visited-over', {str(i):b'' for i in range(129)}, ['scan','fixtures','no-match'], error('visited_limit'))
    for depth in (16,17):
        name='/'.join(['d']*(depth-1)+['a'])
        case(f'depth-{depth}', {name:b''}, ['scan','fixtures','no-match'],
             (0,canonical(manifest([])),b'') if depth==16 else error('depth_limit',name))
    for name in ('../escape','/absolute','a/../b','a//b','a\\b','a:b','./a','a/','a\0b'):
        index=len(cases)
        value=manifest([entry(name,b'')])
        case(f'invalid-path-{index}', {'manifest':canonical(value)}, ['verify','fixtures','manifest'], error('invalid_path',name))
    for label, field, value, problem in [('digest','sha256','Z'*64,'manifest_digest'),
                                        ('size','size',-1,'manifest_size'),
                                        ('size-cap','size',4097,'file_bytes_limit')]:
        row=entry('a',b''); row[field]=value
        case(label, {'manifest':canonical(manifest([row]))}, ['verify','fixtures','manifest'], error(problem,'a'))
    dupe=manifest([entry('a',b''),entry('a',b'')])
    case('duplicate-path', {'manifest':canonical(dupe)}, ['verify','fixtures','manifest'], error('duplicate_path','a'))
    too_many=manifest([entry(str(i),b'') for i in range(65)])
    case('manifest-records', {'manifest':canonical(too_many)}, ['verify','fixtures','manifest'], error('record_limit'))
    # Walk yields links without descending/resolving them. Selected links fail
    # metadata validation; a link/* scan selects nothing. Verify still checks
    # every listed parent component before opening a leaf.
    case('link-file', {'a':b'abc'}, ['scan','fixtures','link'], error('symlink','link'), links={'link':'a'})
    case('link-parent', {'docs/a':b'abc'}, ['scan','fixtures','link/*'], (0,canonical(manifest([])),b''), links={'link':'docs'})
    case('link-outside', {}, ['scan','fixtures','*'], error('symlink','link'), links={'link':str(outside/'secret')})
    case('link-cycle', {}, ['scan','fixtures','*'], error('symlink','link'), links={'link':'.'})
    case('verify-link', {'a':b'abc','manifest':canonical(manifest([entry('link',b'abc')]))},
         ['verify','fixtures','manifest'], error('symlink','link'), links={'link':'a'})
    case('verify-parent-link', {'docs/a':b'abc','manifest':canonical(manifest([entry('link/a',b'abc')]))},
         ['verify','fixtures','manifest'], error('symlink','link/a'), links={'link':'docs'})
    case('manifest-link', {'manifest':blank}, ['verify','fixtures','link'], error('symlink','link'), links={'link':'manifest'})
    case('verify-outside-link', {'manifest':canonical(manifest([entry('link',b'abc')]))},
         ['verify','fixtures','manifest'], error('symlink','link'), links={'link':str(outside/'secret')})
    case('unselected-links', {'a':b'abc'}, ['scan','fixtures','absent'], (0,canonical(manifest([])),b''), links={'link':'a'})

    frozen=snapshot(data); outside_frozen=snapshot(outside)
    grants=output/'grants.json'
    observations=0
    for label, directory, args, expected, grant in cases:
        policy=dict(io=dict(stdout='stdout.bytes@1',stderr='stderr.bytes@1',dirs=[] if grant=='dir' else
                             [dict(path='fixtures',host=str(directory),access='read')]),args=args)
        grants.write_text(json.dumps(policy))
        for engine in ('h','llvm','cranelift'):
            argv=([hirsel,'run',selection,'--max-steps','unbounded','--grants',grants] if engine=='h' else
                  [binaries[engine],'--hirsel-grants',grants,*args])
            actual=run(label+'-'+engine,argv,runtime=True)
            if label=='help':
                assert actual[0]==0 and actual[2]==b'' and actual[1].startswith(b'Usage: filemanifest scan ROOT PATTERN\n'), actual
                if expected[1] is None: expected=(0,actual[1],b'')
            assert actual==expected,(label,engine,actual,expected,str(output))
            observations+=1
        assert frozen==snapshot(data), ('host fixture changes',label)
        assert outside_frozen==snapshot(outside), ('outside grant changes',label)
    # Repeat a realistic scan with the checked-in grants template, rebased only
    # to this temporary package: its host is relative to the grants file.
    policy=json.loads((root/'grants.json').read_text())
    assert policy['io']['dirs'][0]['access']=='read'
    template_args=['verify','fixtures','selected.json']
    for engine in ('h','llvm','cranelift'):
        argv=([hirsel,'run',selection,'--grants',root/'grants.json','--',*template_args] if engine=='h' else
              [binaries[engine],'--hirsel-grants',root/'grants.json',*template_args])
        actual=run('template-'+engine,argv,runtime=True)
        assert actual==(0,canonical(good),b''),(engine,actual)
        observations+=1
    shutil.rmtree(data); shutil.rmtree(outside); grants.unlink()
    assert not data.exists() and not outside.exists() and not grants.exists()
    result=dict(cases=len(cases)+1,observations=observations,upstream_vectors=len(vectors),engines=['h','llvm','cranelift'],cleanup=True)
    (output/'result.json').write_text(json.dumps(result,indent=2)+'\n')
    print(json.dumps(result))


if __name__=='__main__':
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--hirsel',required=True,type=Path)
    parser.add_argument('--root',required=True,type=Path)
    parser.add_argument('--output',required=True,type=Path)
    args=parser.parse_args()
    verify(args.hirsel.resolve(),args.root.resolve(),args.output.resolve())
