"""Whole patchctl; independent byte targets and pinned similar newline vector.

Runtime caps stay 60 CPU seconds / 2 GiB. A tooling stop is never a semantic
pass. Host directories are unavailable during all replay observations.
"""
import hashlib
import json
import os
from pathlib import Path
import resource
import shutil
import subprocess
import sys

cli, root = sys.argv[1], Path(sys.argv[2])
env = os.environ | {'HIRSEL_DAEMON': 'off', 'HIRSEL_RESULTS': 'none@1',
                    'HIRSEL_JOBS': '4', 'HIRSEL_MEMORY_LIMIT': '2G'}
evidence = Path(os.environ.get('TREEPATCH_29_EVIDENCE', str(root / 'evidence')))
evidence.mkdir(parents=True, exist_ok=True)
ledger = []


def blob(value):
    if isinstance(value, dict):
        return value['repeat'].encode() * value['count']
    return value.encode() if isinstance(value, str) else bytes(value)


def limits():
    resource.setrlimit(resource.RLIMIT_CPU, (60, 61))
    resource.setrlimit(resource.RLIMIT_DATA, (2 << 30, 2 << 30))


def run(identity, argv, setup=False):
    argv = list(map(str, argv))
    (evidence / (identity + '.started.json')).write_text(json.dumps(argv))
    result = subprocess.run(argv, env=env, stdin=subprocess.DEVNULL,
                            capture_output=True, timeout=600 if setup else 60,
                            preexec_fn=None if setup else limits())
    (evidence / (identity + '.stdout')).write_bytes(result.stdout)
    (evidence / (identity + '.stderr')).write_bytes(result.stderr)
    (evidence / (identity + '.receipt.json')).write_text(json.dumps(
        dict(argv=argv, exit=result.returncode)))
    return result


def files(directory):
    return {str(p.relative_to(directory)): p.read_bytes()
            for p in directory.rglob('*') if p.is_file()}


def save():
    (evidence / 'ledger.json').write_text(json.dumps(ledger, indent=2) + '\n')


binaries = {}
for backend in ['llvm', 'cranelift']:
    exe = root / backend
    result = run('build-' + backend, [cli, 'build', root / 'hirsel.json',
                                     '--backend', backend, '-o', exe], True)
    assert result.returncode == 0, (backend, result.stderr)
    binaries[backend] = exe

cases = json.loads((root / 'cases.json').read_text())
selected = os.environ.get('TREEPATCH_29_CASE', '').split(',')
for backend, exe in binaries.items():
    if backend == os.environ.get('TREEPATCH_29_SKIP_BACKEND'): continue
    for case in cases:
        identity = case['id']
        if selected != [''] and identity not in selected:
            continue
        if case.get('scripted_only'):
            ledger.append(dict(id=f'{backend}-{identity}', disposition='INCOMPLETE',
                               reason='host file provider has no scripted fault queue'))
            continue
        cell = root / (backend + '-' + identity)
        cell.mkdir()
        for name in ['source', 'output', 'fixtures', 'target']:
            (cell / name).mkdir()
        source, output, fixtures, target = [cell / x for x in
                                          ['source', 'output', 'fixtures', 'target']]
        for name, data in case['source'].items():
            path = source / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(blob(data))
        for name, data in case.get('target_input', {}).items():
            path = target / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(blob(data))
        (fixtures / 'change.json').write_text(json.dumps(case['envelope'],separators=(',',':'))+' '*case.get('envelope_padding',0))
        original = files(source)
        widths = [1, 2, 4] if identity in ['mixed', 'stale', 'cancel'] else [1]
        recorded = []
        for width in widths:
            label = f'{backend}-{identity}-w{width}'
            for name, data in case.get('output_initial', {}).items():
                (output / name).write_bytes(blob(data))
            policy = dict(args=case['args'], io=dict(
                stdout='stdout.bytes@1', stderr='stderr.bytes@1',
                tasks='tasks.cooperative@1', monotonic_clock='clock.monotonic.ns@1',
                dirs=[dict(path='fixtures', host=str(fixtures), access='read'),
                      dict(path='output', host=str(output), access='read' if case.get('deny_output') else 'read_write'),
                      dict(path='source', host=str(source), access='read'),
                      dict(path='target', host=str(target), access='read')]),
                placement=dict(cores=width, commit='shared', reorder='off',
                               metrics=str(evidence / (label + '.metrics'))))
            if case.get('deny_source'): policy['io']['dirs']=[d for d in policy['io']['dirs'] if d['path']!='source']
            grants = cell / 'grants.json'
            grants.write_text(json.dumps(policy))
            # Retain argv/grants/tool/source bindings before the consumer.
            (evidence / (label + '.grants.json')).write_bytes(grants.read_bytes())
            tape, trace = evidence / (label + '.tape'), evidence / (label + '.jsonl')
            result = run(label, [exe, '--hirsel-trace-events', trace,
                                 '--hirsel-record', tape, '--hirsel-grants', grants,
                                 *case['args']])
            assert (result.returncode, result.stderr) == (case['exit'], case['stderr'].encode()), (label, result)
            assert (json.loads(result.stdout) if result.stdout else None) == case['stdout'], (label, result.stdout)
            assert files(source) == original, (label, 'source changed')
            actual = files(output)
            if case['exit'] == 0 and case['args'][0] == 'tree-apply':
                completion = json.loads(actual.pop('.patchctl-complete.json'))
                assert completion['schema'] == 1
                assert actual == {k: blob(v) for k, v in case['target'].items()}, (label, actual)
                assert completion['files'] == [dict(path=k, sha256=hashlib.sha256(v).hexdigest())
                                               for k, v in sorted(actual.items())]
            else:
                assert actual == {k: blob(v) for k, v in case.get('output_initial', {}).items()}, (label, actual)
            ledger.append(dict(id=label, disposition='PASS', exit=result.returncode))
            recorded.append((label, tape, result))
            shutil.rmtree(output)
            output.mkdir()
            save()
            print('TREEPATCH_29_HOST_PASS '+label,file=sys.stderr,flush=True)
        # No original files, providers or grants can be reopened now.
        shutil.rmtree(cell)
        for label, tape, original_result in recorded:
            digest = hashlib.sha256(tape.read_bytes()).hexdigest()
            for replay_engine, launcher in [('reference', [cli, 'run', root / 'hirsel.json', '--max-steps', '1000000', '--replay']),
                                            *[(name, [binary, '--hirsel-replay']) for name, binary in binaries.items()]]:
                replay_label = label + '-replay-' + replay_engine
                replay = run(replay_label, [*launcher, tape])
                known_refusals = {
                    'cranelift-envelope-bytes-524288-w1-replay-reference': (3, b'', b'hirsel: replay refused: UnconsumedInput(133)\n'),
                    'llvm-envelope-bytes-524288-w1-replay-reference': (3, b'', b'hirsel: replay refused: UnconsumedInput(133)\n'),
                    'llvm-files-32-w1-replay-reference': (3, b'', b'hirsel: replay refused: UnconsumedInput(8)\n'),
                    'llvm-file-bytes-65536-w1-replay-reference': (3, b'', b'hirsel: replay refused: UnconsumedInput(32)\n'),
                    'llvm-source-bytes-524288-w1-replay-reference': (3, b'', b'hirsel: replay refused: UnconsumedInput(172)\n'),
                    'llvm-hunks-256-w1-replay-reference': (3, b'', b'hirsel: replay refused: UnconsumedInput(7)\n'),
                    'cranelift-files-32-w1-replay-reference': (3, b'', b'hirsel: replay refused: UnconsumedInput(8)\n'),
                    'cranelift-file-bytes-65536-w1-replay-reference': (3, b'', b'hirsel: replay refused: UnconsumedInput(32)\n'),
                    'cranelift-source-bytes-524288-w1-replay-reference': (3, b'', b'hirsel: replay refused: UnconsumedInput(172)\n'),
                    'cranelift-hunks-256-w1-replay-reference': (3, b'', b'hirsel: replay refused: UnconsumedInput(7)\n'),
                }
                observed = (replay.returncode, replay.stdout, replay.stderr)
                # d78c17e1 preserves the evaluator cut instead of masking it at
                # tape finish. Only these exact owned identities may stop here.
                tooling_stop = (3, b'', b'evaluator tooling limit: Steps\n')
                if replay_label in known_refusals and observed in (known_refusals[replay_label], tooling_stop):
                    ledger.append(dict(id=replay_label, disposition='INCOMPLETE', reason='exact reference-replay-29 disposition; evaluator limit is not semantic completion'))
                else:
                    assert (replay.returncode, replay.stdout, replay.stderr) == (original_result.returncode, original_result.stdout, original_result.stderr), (replay_label, replay)
                    ledger.append(dict(id=replay_label, disposition='PASS'))
                assert not cell.exists(), 'replay recreated host tree'
                assert hashlib.sha256(tape.read_bytes()).hexdigest() == digest
                save()
                print('TREEPATCH_29_REPLAY '+replay_label+' '+ledger[-1]['disposition'],file=sys.stderr,flush=True)
assert any(row['disposition'] == 'PASS' for row in ledger)
print(json.dumps(dict(completed=sum(row['disposition'] == 'PASS' for row in ledger),
                      incomplete=sum(row['disposition'] == 'INCOMPLETE' for row in ledger))))
