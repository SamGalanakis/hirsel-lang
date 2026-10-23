"""Bounded full-application native checks, independent sequential bytes.find oracle.

cases.json ports Go bytes Index short-string vectors (ebe2feefc400f813,
src/bytes/bytes_test.go:143-184, BSD-3-Clause, The Go Authors).
Empty query is deliberately rejected; repeated matches advance by query bytes.
"""
import hashlib
import json
import os
from pathlib import Path
import resource
import subprocess
import sys


def verify(hirsel, root, backend):
    hirsel = str(Path(hirsel).resolve())
    env = {k: v for k, v in os.environ.items() if not k.startswith('HIRSEL_RT_')}
    env.update(HIRSEL_JOBS='1', HIRSEL_MEMORY_LIMIT='2G', HIRSEL_DAEMON='off')
    (root / 'environment.json').write_text(json.dumps({k: v for k, v in env.items() if k.startswith('HIRSEL_')}, indent=2) + '\n')
    commands = []

    def run(label, argv, runtime=False):
        argv = list(map(str, argv))
        def limits():
            resource.setrlimit(resource.RLIMIT_CPU, (60, 60))
        actual = [sys.executable, root / 'capped', '--timeout', '60' if runtime else '300', *argv]
        done = subprocess.run(list(map(str, actual)), stdin=subprocess.DEVNULL,
                              capture_output=True, env=env, cwd=root,
                              timeout=90 if runtime else 300,
                              preexec_fn=limits if runtime else None)
        (root / (label + '.stdout')).write_bytes(done.stdout)
        (root / (label + '.stderr')).write_bytes(done.stderr)
        commands.append(dict(id=label, argv=list(map(str, actual)), status=done.returncode))
        (root / 'commands.json').write_text(json.dumps(commands, indent=2) + '\n')
        return done

    executable = root / backend
    built = run('build', [hirsel, 'build', root / 'hirsel.json', '--backend', backend, '-o', executable])
    assert built.returncode == 0, built.stderr.decode(errors='replace')
    cases = json.loads((root / 'cases.json').read_text())
    completed = 0
    for case in cases:
        directory = root / case['id']
        directory.mkdir()
        for name, data in case['files'].items():
            value = (data['repeat'].encode() * data['count'] if isinstance(data, dict)
                     else data.encode() if isinstance(data, str) else bytes(data))
            (directory / name).write_bytes(value)
        before = {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in directory.iterdir()}
        # The expected literal rows were sealed before engine execution. Check
        # successful fixtures again using Python's independent byte search.
        if case['status'] == 0:
            rows = []
            query = case['query'].encode()
            for index, name in enumerate(case['paths']):
                data = (directory / name).read_bytes()
                start = 0
                while len(rows) < case['limit']:
                    at = data.find(query, start)
                    if at < 0:
                        break
                    rows.append(f'{index}:{at}\n')
                    start = at + len(query)
                if len(rows) == case['limit']:
                    break
            assert ''.join(rows) == case['stdout'], case['id']
        for jobs in case.get('jobs', (1, 2, 4)):
            label = f"{case['id']}-j{jobs}"
            grant = dict(args=['fixtures', case['query'], str(jobs), str(case['limit']), *case['paths']],
                         io=dict(stdout='stdout.bytes@1', stderr='stderr.bytes@1',
                                 tasks='tasks.cooperative@1',
                                 dirs=[dict(path='fixtures', host=str(directory), access='read')]),
                         placement=dict(cores=1, commit='shared', reorder='auto'))
            grants = root / (label + '.grants.json')
            grants.write_text(json.dumps(grant) + '\n')
            # Seal every input actually consumed by this native invocation,
            # including the newly compiled executable and instantiated grants.
            inputs = [executable, grants, root / 'main.hir', root / 'hirsel.json',
                      root / 'cases.json', root / 'verify.py', root / 'capped',
                      Path(hirsel), *directory.iterdir()]
            (root / (label + '.inputs.json')).write_text(json.dumps({
                str(p): hashlib.sha256(p.read_bytes()).hexdigest() for p in inputs
            }, indent=2) + '\n')
            # Capture target streams separately from tools/capped's report.
            target_out = root / (label + '.target.stdout')
            target_err = root / (label + '.target.stderr')
            child = root / 'child.py'
            child.write_text('import pathlib,subprocess,sys\np=subprocess.run(sys.argv[3:],capture_output=True)\npathlib.Path(sys.argv[1]).write_bytes(p.stdout)\npathlib.Path(sys.argv[2]).write_bytes(p.stderr)\nsys.exit(p.returncode)\n')
            result = run(label, [sys.executable, child, target_out, target_err, executable,
                                '--hirsel-record', root / (label + '.tape'),
                                '--hirsel-grants', grants, *grant['args']], runtime=True)
            assert result.returncode == case['status'], (label, result.returncode, result.stderr, target_err.read_bytes() if target_err.exists() else '')
            assert target_out.read_bytes() == case['stdout'].encode(), label
            assert target_err.read_bytes() == case['stderr'].encode(), (label, target_err.read_bytes())
            assert before == {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in directory.iterdir()}, label
            completed += 1
            print(json.dumps(dict(id=case['id'], engine=backend, jobs=jobs, completed=1)), flush=True)
    assert completed == sum(len(c.get('jobs', (1, 2, 4))) for c in cases)
    print(json.dumps(dict(engine=backend, passed=completed, cleanup=True)), flush=True)


if __name__ == '__main__':
    verify(sys.argv[1], Path(sys.argv[2]), sys.argv[3])
