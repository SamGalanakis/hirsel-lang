"""Independent Python csv/json oracle and bounded cap/one-over input recipes.
Go ebe2feef reader_test.go:50-100 supplies complete-record/CRLF/multiline vectors.
"""
import copy
import csv
import io
import json
import pathlib


def compact(value):
    return json.dumps(value, ensure_ascii=False, separators=(',', ':'))


def oracle(config, inputs):
    data, issues, groups = [], [], {}
    seen = filtered = rejected = 0
    for file_index, (fmt, payload) in enumerate(inputs):
        if fmt == 'csv':
            reader = csv.reader(io.StringIO(payload, newline=''))
            names = next(reader)
            raw = [dict(zip(names, row)) if len(row) == len(names) else None for row in reader if row]
        else:
            raw = [json.loads(line) for line in payload.splitlines()]
        for ordinal, obj in enumerate(raw, 1):
            seen += 1
            code, field = None, 'row'
            cells = {}
            if not isinstance(obj, dict):
                code = 'object'
            elif any(k not in [f['name'] for f in config['fields']] for k in obj):
                code, field = 'extra', next(k for k in obj if k not in [f['name'] for f in config['fields']])
            else:
                for spec in config['fields']:
                    key = spec['name']
                    if key not in obj:
                        if 'default' in spec:
                            v = spec['default']
                        elif spec['required']:
                            code, field = 'required', key
                            break
                        else:
                            cells[key] = None
                            continue
                    else:
                        v = obj[key]
                        if fmt == 'csv':
                            if spec['type'] == 'integer':
                                try:
                                    v = int(v, 10)
                                except ValueError:
                                    pass
                            elif spec['type'] == 'bool' and v in ('true', 'false'):
                                v = v == 'true'
                    typ = {'string': str, 'integer': int, 'bool': bool}[spec['type']]
                    if type(v) is not typ or (typ is int and not -(2**63) <= v < 2**63):
                        code, field = 'type', key
                        break
                    cells[key] = v
            stage = 'wire'
            if code is None and any(cells[f['field']] != f['equals'] for f in config.get('filters', [])):
                filtered += 1
                continue
            if code is None and 'group' in config:
                g = config['group']
                key, value = cells[g['key']], cells[g['sum']]
                total = groups.get(key, 0) + value
                if not -(2**63) <= total < 2**63:
                    code, field, stage = 'overflow', 'group', 'transform'
                else:
                    groups[key] = total
            if code:
                rejected += 1
                if len(issues) < 32:
                    issues.append(dict(file=file_index, row=ordinal, field=field, code=code, stage=stage))
                continue
            data.append({f.get('as', f['name']): cells[f['name']] for f in config['fields'] if f.get('project', True)})
    rendered = ''.join(compact(row) + '\n' for row in data)
    summary = dict(rows=seen, accepted=len(data), filtered=filtered, rejected=rejected,
                   issues_truncated=rejected > len(issues), issues=issues,
                   groups=[dict(key=k, sum=v) for k, v in sorted(groups.items())])
    return rendered, compact(summary) + '\n'


def cases():
    fixture = pathlib.Path(__file__).resolve().parent / 'fixtures'
    # Tests also copy the actual fixtures beside this script in a detached package.
    if not fixture.exists():
        fixture = pathlib.Path(__file__).with_name('fixtures')
    base = json.loads((fixture / 'pipeline.json').read_text())
    out = []

    def add(id, payloads, config=None, error=None, command='run', pending=None, fault=False):
        config = copy.deepcopy(base if config is None else config)
        entries = [dict(path=f'f{i}.{fmt}', format=fmt) for i, (fmt, _) in enumerate(payloads)]
        files = {e['path']: list(p.encode() if isinstance(p, str) else p) for e, (_, p) in zip(entries, payloads)}
        case = dict(id=id, manifest=dict(inputs=entries), pipeline=config, files=files,
                    command=command, exit=1 if error else 0, stdout='', error=error,
                    steps=8000000, pending=pending, fault=fault)
        if error is None and command == 'run':
            case['data'], case['summary'] = oracle(config, payloads)
            case['stdout'] = 'complete\n'
        elif error is None:
            case['stdout'] = 'valid\n' if command == 'check' else f'files={len(entries)} fields={len(config["fields"])} credits={config["jobs"]} order=manifest,row groups=sorted completion=manifest\n'
        out.append(case)
        return case

    mixed = [('csv', (fixture / 'a.csv').read_text()), ('jsonl', (fixture / 'b.jsonl').read_text())]
    add('mixed-success', mixed)
    add('check', mixed, command='check')
    add('explain', mixed, command='explain')
    plain = dict(fields=[dict(name='a', type='string', required=True), dict(name='b', type='string', required=True), dict(name='c', type='string', required=True)], policy='report-and-skip', jobs=4)
    add('go-simple', [('csv', 'a,b,c\na,b,c\n')], plain)
    two = copy.deepcopy(plain); two['fields'] = two['fields'][:2]
    add('go-crlf', [('csv', 'a,b\r\na,b\r\nc,d\r\n')], two)
    add('go-rfc4180', [('csv', 'a,b,c\n"aaa","bb\nb","ccc"\n"a,a","b""bb","ccc"\nzzz,yyy,xxx\n')], plain)
    add('go-multiline', [('csv', 'a,b,c\n"two\nline","one line","three\nline\nfield"')], plain)
    add('field-count', [('csv', 'a,b,c\nx,y\nx,y,z\n')], plain)
    add('missing-header', [('csv', 'amount\n3\n')], error='source: missing header')
    add('duplicate-header', [('csv', 'team,team\nx,x\n')], error='source: duplicate header')
    add('extra-header', [('csv', 'team,amount,other\nx,3,z\n')], error='source: extra header')
    add('utf8', [('jsonl', b'\xff')], error='source: utf8')
    add('json', [('jsonl', '{broken\n')], error='source: jsonl')
    add('missing-extra-type-default', [('jsonl', '{"team":"a","amount":2}\n{"team":"x"}\n{"team":"y","amount":3,"extra":0}\n{"team":"z","amount":"4"}\n')])
    add('overflow', [('jsonl', '{"team":"a","amount":9223372036854775807}\n{"team":"a","amount":1}\n')])
    add('issues-32', [('jsonl', '{}\n' * 32)])
    add('issues-33', [('jsonl', '{}\n' * 33)])
    fast = copy.deepcopy(base); fast['policy'] = 'fail-fast'
    for gate in ['Pending', {'AfterWrite': {'path': 'never'}}]:
        add('pending-cancel' if gate == 'Pending' else 'afterwrite-cancel', [('jsonl', '{"team":"a","amount":1}\n'), ('jsonl', '{}\n')], fast, error='row: validation', pending=gate)
    add('sink-failure', mixed, error='output: write', fault=True)
    add('files-8', [('jsonl', '')] * 8)
    add('files-9', [('jsonl', '')] * 9, error='limit: files')
    one = dict(fields=[dict(name='x', type='string', required=True)], policy='report-and-skip', jobs=4)
    add('bytes-65536', [('csv', 'x\n' + ('v\n' * 1023) + ('\n' * (65536 - 2048)))], one)
    add('bytes-65537', [('csv', 'x\n' + '\n' * 65535)], one, error='limit: file bytes')
    add('rows-1024', [('jsonl', '{"x":"v"}\n' * 1024)], one)
    add('rows-1025', [('jsonl', '{"x":"v"}\n' * 1025)], one, error='limit: rows')
    for n in [16, 17]:
        config = copy.deepcopy(one); config['fields'] = [dict(name=f'x{i}', type='string', required=True) for i in range(n)]
        add(f'fields-{n}', [('jsonl', compact({f'x{i}': 'v' for i in range(n)}) + '\n')], config, error='limit: fields' if n == 17 else None)
    add('record-4096', [('csv', 'x\n' + 'a' * 4096 + '\n')], one)
    add('record-4097', [('csv', 'x\n' + 'a' * 4097 + '\n')], one, error='limit: record bytes')
    for n in [64, 65]:
        add(f'groups-{n}', [('jsonl', ''.join(compact(dict(team=str(i), amount=1)) + '\n' for i in range(n)))], error='limit: groups' if n == 65 else None)
    over = copy.deepcopy(base); over['jobs'] = 5
    add('credits-5', mixed, over, error='limit: credits')
    # Output record {"x":"..."}\n is payload+9 bytes. Exact independent budget.
    for n in [262144, 262145]:
        size = n - 9 * 66
        lengths = [4000] * 65 + [size - 260000]
        payload = ''.join(compact(dict(x='a' * k)) + '\n' for k in lengths)
        # Split files to keep every file within 64 KiB.
        lines = payload.splitlines(keepends=True)
        chunks = [('jsonl', ''.join(lines[i:i+13])) for i in range(0, len(lines), 13)]
        add(f'output-{n}', chunks, one, error='limit: output bytes' if n > 262144 else None)
    projected = copy.deepcopy(base)
    projected['fields'][2]['project'] = False
    add('projection', mixed, projected)
    optional = copy.deepcopy(base)
    optional['fields'][2].pop('default')
    optional.pop('filters')
    add('optional-null', [('jsonl', '{"team":"a","amount":2}\n')], optional)
    # Multiple refills/checks in file zero let a later short file finish first.
    reorder = copy.deepcopy(base)
    add('reorder', [('jsonl', ''.join(compact(dict(team='a' * 300, amount=i)) + '\n' for i in range(24))), ('jsonl', '{"team":"b","amount":9}\n')], reorder)
    return out


if __name__ == '__main__':
    print(compact(cases()))
