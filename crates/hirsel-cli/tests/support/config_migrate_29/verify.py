#!/usr/bin/env python3
"""Independent tomllib/patch oracle and bounded production Ordered launches."""
import argparse
import copy
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tomllib


def expected(source, target):
    value = tomllib.loads(source)
    if value['version'] == 1:
        timeout = value.pop('timeout_ms')
        value['service'] = dict(endpoint=value.pop('endpoint'))
        value['timeouts'] = dict(connect_ms=min(timeout, 1000), request_ms=timeout)
        value['version'] = 2
    if value['version'] == 2 and target == 3:
        count = value.pop('retries')
        value['retry'] = dict(attempts=count, policy='none' if count == 0 else 'fixed')
        value['version'] = 3
    return value


def patch(source, diff):
    """Strict unified patch consumer, including Similar's missing-newline vector."""
    if not diff:
        return source
    lines = diff.splitlines(keepends=True)
    assert lines[0].startswith('--- ') and lines[1].startswith('+++ ')
    old = source.splitlines(keepends=True)
    output, cursor, i = [], 0, 2
    while i < len(lines):
        match = re.fullmatch(r'@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@\n', lines[i])
        assert match, lines[i]
        start, deleted, new, inserted = (int(x) if x is not None else 1 for x in match.groups())
        start = start - 1 if deleted else start
        assert start >= cursor
        output.extend(old[cursor:start]); cursor = start; i += 1
        removed, added = 0, 0
        while i < len(lines) and not lines[i].startswith('@@ '):
            marker, content = lines[i][0], lines[i][1:]; i += 1
            if i < len(lines) and lines[i] == '\\ No newline at end of file\n':
                content = content[:-1]; i += 1
            assert marker in ' +-'
            if marker in ' -':
                assert cursor < len(old) and old[cursor] == content, (old, cursor, content)
                cursor += 1; removed += 1
            if marker in ' +':
                output.append(content); added += 1
        assert (removed, added) == (deleted, inserted)
    return ''.join(output + old[cursor:])


def oracle(root, case, plan=False):
    directory = root / case['id']
    output = (directory / 'stdout').read_bytes()
    if case.get('deny'):
        assert output == b'' and not list(directory.glob('*.toml'))
        return
    if case.get('output_fail'):
        assert (directory / 'manifest.json').exists()
        return
    rows = [json.loads(line) for line in output.splitlines()]
    assert len(rows) == len(case['files']), (case['id'], rows)
    for i, (row, source, error) in enumerate(zip(rows, case['files'], case['errors'])):
        assert row['source'] == f'{i}.toml'
        destination = directory / f'{i}.toml'
        status = 'failed' if error else 'write_failed' if case.get('write_fail') else 'planned' if plan else 'published'
        assert row['status'] == status, (case['id'], row)
        if error:
            assert row['error'] == error and not destination.exists()
            continue
        if case.get('write_fail'):
            assert not destination.exists()
            continue
        text = patch(source, row['diff'])
        assert tomllib.loads(text) == expected(source, case['target']), (case['id'], text)
        # Unknown quoted keys, arrays of tables, comments and unchanged lines.
        if '"app.name"' in source:
            assert '# app-owned\n"app.name" = "wool"\n[[workers]] # pool\nname = "one"\n' in text
        if tomllib.loads(source)['version'] == case['target']:
            assert text == source and row['diff'] == '' and row['changes'] == []
        if not plan:
            assert destination.read_text() == text
    if not plan:
        manifest = json.loads((directory / 'manifest.json').read_text())
        assert manifest == dict(schema=1, target=case['target'], success=case['exit'] == 0, files=rows)


def main(args):
    root = args.root
    cases = json.loads((root / 'cases.json').read_text())
    if args.oracle:
        oracle(root, next(c for c in cases if c['id'] == args.oracle), plan=args.plan)
        return
    # File-fault scripting is a SimulationConfig seam. Native file providers
    # are real confined files; unsupported injection cells stay explicitly unrun.
    cases = [c for c in cases if not c.get('fault')]
    if os.getenv('CONFIG_MIGRATE_29_CASE'):
        cases = [c for c in cases if c['id'] in os.environ['CONFIG_MIGRATE_29_CASE'].split(',')]
    assert cases
    env = dict(os.environ, HIRSEL_DAEMON='off', HIRSEL_RESULTS='none@1', HIRSEL_JOBS='4')
    binaries = {}
    receipts = []
    for backend in ['llvm', 'cranelift']:
        binary = root / ('app-' + backend)
        if os.getenv('CONFIG_MIGRATE_29_REUSE_BINARIES'):
            original = Path(os.environ['CONFIG_MIGRATE_29_REUSE_BINARIES']) / ('app-' + backend)
            shutil.copy2(original, binary)
            assert hashlib.sha256(binary.read_bytes()).digest() == hashlib.sha256(original.read_bytes()).digest()
        else:
            build = subprocess.run([str(args.hirsel), 'build', str(root/'hirsel.json'), '--backend', backend, '-o', str(binary)], env=env, capture_output=True, timeout=300)
            (root/(backend+'-build.stdout')).write_bytes(build.stdout)
            (root/(backend+'-build.stderr')).write_bytes(build.stderr)
            assert build.returncode == 0, (backend, build)
        binaries[backend] = binary
    frozen = list(root.glob('*.hir')) + [root/'hirsel.json', root/'cases.json', root/'caps.json', root/'verify.py', args.hirsel, *binaries.values()]
    std = Path(json.loads((root/'hirsel.json').read_text())['dependencies'][0]['manifest']).parent
    frozen.extend(std.rglob('*'))
    for variable in ['HIRSEL_RUNTIME_ARCHIVES', 'HIRSEL_LINK_KIT', 'LLVM_SYS_231_PREFIX']:
        if os.getenv(variable): frozen.extend(Path(os.environ[variable]).rglob('*'))
    (root/'preexecution-seal.json').write_text(json.dumps({str(p):hashlib.sha256(p.read_bytes()).hexdigest() for p in frozen if p.is_file()},indent=2))
    runenv = dict(env, HIRSEL_MEMORY_LIMIT='2G')
    def run(command, label):
        row = dict(id=label, argv=list(map(str,command)), state='started')
        receipts.append(row); (root/'receipts.json').write_text(json.dumps(receipts,indent=2))
        done = subprocess.run(['python3',str(root/'capped'),'--timeout','60','prlimit','--cpu=60:61',*map(str,command)], env=runenv, capture_output=True, timeout=90)
        # capped diagnostics belong to the host wrapper, not app stderr.
        (root/(label+'.stdout')).write_bytes(done.stdout)
        (root/(label+'.stderr')).write_bytes(done.stderr)
        row.update(state='completed',exit=done.returncode,stdout_sha256=hashlib.sha256(done.stdout).hexdigest())
        (root/'receipts.json').write_text(json.dumps(receipts,indent=2))
        return done
    tapes = []
    for case in cases:
        id = case['id']; data = root/(id+'-input'); data.mkdir()
        destination = root/(id+'-output'); destination.mkdir()
        for i, text in enumerate(case['files']):
            if text is not None: (data/f'{i}.toml').write_text(text)
        operands = [f'{i}.toml' for i in range(len(case['files']))]
        for backend,binary in binaries.items():
            for width in ([1,2,4] if id in ["v1-v3", "mixed"] else [1]):
                label=f'{backend}-{id}-{width}'
                for p in destination.iterdir(): p.unlink()
                io=dict(stdout='stdout.bytes@1',tasks='tasks.cooperative@1',dirs=[dict(path='configs',host=str(data),access='read'),dict(path='migrated',host=str(destination),access='read_write')])
                if case.get('deny'):io.pop('tasks')
                grants=root/(label+'-grants.json')
                grants.write_text(json.dumps(dict(io=io,args=[],placement=dict(cores=width,commit='shared',reorder='auto',metrics=str(root/(label+'-metrics.json'))))))
                tape=root/(label+'.tape');trace=root/(label+'.jsonl')
                argv=[case.get('command','toml-migrate'),'--to',str(case['target']),'--jobs','4',*operands]
                recorded=run([binary,*([] if case.get('deny') else ['--hirsel-trace-events',trace]),'--hirsel-record',tape,'--hirsel-grants',grants,*argv],label)
                assert recorded.returncode == case['exit'], (label,recorded)
                check=root/id;check.mkdir(exist_ok=True)
                for p in check.iterdir(): p.unlink()
                (check/'stdout').write_bytes(recorded.stdout)
                for p in destination.iterdir(): shutil.copy(p,check/p.name)
                oracle(root,case,plan=case.get("command")=="toml-plan")
                tapes.append(dict(case=id,backend=backend,width=width,path=str(tape)))
                # Replay with original mounted directories and grant unavailable.
                parked_data=root/(id+'-offline-input');data.rename(parked_data)
                parked_output=root/(id+'-offline-output');destination.rename(parked_output)
                grants.rename(grants.with_suffix('.offline'))
                for replay_engine in ['reference','llvm','cranelift']:
                    command=[args.hirsel,'run',root/'hirsel.json','--replay',tape] if replay_engine=='reference' else [binaries[replay_engine],'--hirsel-replay',tape]
                    replay=run(command,label+'-replay-'+replay_engine)
                    assert replay.returncode==recorded.returncode and replay.stdout==recorded.stdout, (label,replay_engine,replay)
                    app_stderr=lambda raw: b''.join(line for line in raw.splitlines(keepends=True) if not line.startswith(b'capped:'))
                    assert app_stderr(replay.stderr)==app_stderr(recorded.stderr),(label,replay_engine,'stderr',replay)
                    assert not data.exists() and not destination.exists() and not grants.exists()
                parked_data.rename(data);parked_output.rename(destination)
                # Same-target migration reads the published bytes and must be exact.
                if case['exit']==0 and width==1 and case.get('command')!='toml-plan':
                    before={p.name:p.read_bytes() for p in destination.glob('*.toml')}
                    repeat_dir=root/(label+'-repeat');repeat_dir.mkdir()
                    repeat_grant=root/(label+'-repeat.json')
                    repeat_grant.write_text(json.dumps(dict(args=[],io=dict(stdout='stdout.bytes@1',tasks='tasks.cooperative@1',dirs=[dict(path='configs',host=str(destination),access='read'),dict(path='migrated',host=str(repeat_dir),access='read_write')]))))
                    repeated=run([binary,'--hirsel-grants',repeat_grant,*argv],label+'-repeat')
                    assert repeated.returncode==0 and before=={p.name:p.read_bytes() for p in repeat_dir.glob('*.toml')}
        shutil.rmtree(data);shutil.rmtree(destination)
    # Actual cap and one-over files, once per backend at width 1. These are
    # complete app runs; a resource cap remains a finding at unchanged limits.
    caps = [] if os.getenv('CONFIG_MIGRATE_29_SKIP_CAPS') else json.loads((root/'caps.json').read_text())
    source = 'version=3\nlog_level="info"\nservice.endpoint="x"\ntimeouts={connect_ms=1,request_ms=1}\nretry={attempts=0,policy="none"}\n'
    for cap in caps:
        data=root/(cap['id']+'-caps'); data.mkdir()
        files=[]
        for i in range(cap['count']):
            size=cap.get('last_bytes',cap['bytes']) if i+1==cap['count'] else cap['bytes']
            text=source+' '*(size-len(source)) if size>=len(source) else ' '*size
            name=f'{i}.toml'; (data/name).write_text(text);files.append(name)
        grant=root/(cap['id']+'-caps.json')
        grant.write_text(json.dumps(dict(args=[],io=dict(stdout='stdout.bytes@1',tasks='tasks.cooperative@1',dirs=[dict(path='configs',host=str(data),access='read')]),placement=dict(cores=1,commit='shared',reorder='auto'))))
        for backend,binary in binaries.items():
            label=backend+'-'+cap['id']
            done=run([binary,'--hirsel-grants',grant,'toml-plan','--to','3','--jobs',str(cap.get('jobs',4)),*files],label)
            assert done.returncode==cap['exit'],(label,done)
            if cap['exit']==0:
                rows=[json.loads(line) for line in done.stdout.splitlines()]
                assert len(rows)==cap['count'] and all(row['status']=='planned' and row['diff']=='' and row['changes']==[] for row in rows)
            else: assert not done.stdout
    (root/'tapes.json').write_text(json.dumps(tapes,indent=2))
    # Corrupt/truncated tape must refuse rather than claim a replay pass.
    tape=Path(tapes[0]['path']);damaged=root/'truncated.tape';damaged.write_bytes(tape.read_bytes()[:-1])
    for engine in ['reference','llvm','cranelift']:
        command=[args.hirsel,'run',root/'hirsel.json','--replay',damaged] if engine=='reference' else [binaries[engine],'--hirsel-replay',damaged]
        refused=run(command,'truncated-'+engine)
        assert refused.returncode==3 and not refused.stdout
    print(json.dumps(dict(completed=len(receipts),tapes=len(tapes),unsupported=['native file-fault/cancellation injection','checked-H host-tape replay'])))


if __name__=='__main__':
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('--root',type=Path,required=True)
    p.add_argument('--oracle')
    p.add_argument('--plan',action='store_true')
    p.add_argument('--native')
    p.add_argument('--hirsel',type=Path)
    main(p.parse_args())
