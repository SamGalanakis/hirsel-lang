#!/usr/bin/env python3
"""RPC admission invariants over normalized committed transitions.

The finite vectors use semantic milliseconds. Live evidence needs an independent
runtime witness; output rows and replay identity alone cannot supply one.
Adapted cancellation/expired-entry vectors follow cenkalti/backoff at
6f228bfdb51209aa375869feb27dcede4fa0db06, retry_test.go:89-124,393-411.
Hirsel deliberately retains fixed 10/20ms delays and zero-attempt expired entry.
"""
import json
import sys
from pathlib import Path

INVARIANTS = frozenset('structure identity admission deadline_binding request_credit slots attempt_sequence attempt_cap deadline cancellation retry_safety backoff correlation terminal join report replay'.split())
RETRY = {'refused', 'reset', 'abort', 'broken_pipe', 'net_timeout', 'eof'}
RESULTS = RETRY | {'ok', 'application', 'protocol', 'wrong_id', 'grant', 'deadline', 'canceled', 'unknown'}

class Violation(Exception):
    pass

def check(v):
    covered = set()
    def require(ok, rule):
        if not ok:
            raise Violation(rule)
        covered.add(rule)
    cfg = v['config']
    require(1 <= cfg['jobs'] <= 4 and cfg['request_ms'] == 100 and len(v['events']) <= 1024, 'structure')
    req, seen, disconnected, rows = {}, set(), set(), []
    slots = set()
    previous = -1
    joined = False
    for e in v['events']:
        t, op = e['t'], e['op']
        require(type(t) is int and t >= previous and not joined, 'structure')
        previous = t
        key = (e.get('c', 0), e.get('r', 0))
        require(key[0] == 0 and 0 <= key[1] < 64, 'structure')
        if op in ('reserve', 'busy'):
            require(key not in seen and len(seen) < 64, 'identity')
            require(key[0] not in disconnected, 'cancellation')
            held = sum(r['credit'] for r in req.values())
            require((held == 16) if op == 'busy' else (held < 16), 'admission')
            seen.add(key)
            if op == 'busy':
                rows.append(dict(client=key[0], request=key[1], attempt=0, status='busy'))
            else:
                require(e['id'] not in [r['id'] for k, r in req.items() if k[0] == key[0]], 'identity')
                req[key] = dict(id=e['id'], safe=e['safe'], credit=True, deadline=None, entered=False,
                                number=0, row=None, issued=False, result=None, cancel=None,
                                terminal=False, wake=None, ready=False, connected=False)
            continue
        if op == 'disconnect':
            disconnected.add(key[0])
            for k, r in req.items():
                if k[0] == key[0] and not r['terminal']:
                    r['cancel'] = 'disconnect'
            continue
        if op == 'join':
            require(not slots and all(not r['credit'] and r['terminal'] for r in req.values()), 'join')
            joined = True
            continue
        require(key in req, 'identity')
        r = req[key]
        if op == 'admit':
            require(r['deadline'] is None and e['deadline'] == t + cfg['request_ms'], 'deadline_binding')
            r['deadline'] = e['deadline']
        elif op == 'enter':
            require(r['deadline'] is not None and t < r['deadline'], 'deadline')
            require(not r['cancel'] and not r['entered'], 'cancellation')
            r['entered'] = True
        elif op == 'acquire':
            require(r['credit'] and r['entered'] and not r['terminal'], 'request_credit')
            require(key not in slots and len(slots) < cfg['jobs'], 'slots')
            slots.add(key)
        elif op == 'begin':
            require(e['a'] <= 3, 'attempt_cap')
            require(e['a'] == r['number'] + 1 and r['row'] is None, 'attempt_sequence')
            require(key in slots, 'slots')
            if r['number']:
                require(r['safe'] and r['result'] in RETRY, 'retry_safety')
                require(r['ready'], 'backoff')
            r.update(number=e['a'], result=None, issued=False, connected=False, ready=False)
            rows.append(dict(client=key[0], request=key[1], attempt=e['a'], status='running'))
            r['row'] = len(rows) - 1
        elif op == 'issue':
            require(not r['cancel'] and key[0] not in disconnected and not r['terminal'], 'cancellation')
            require(t < min(r['deadline'], cfg['session_ms']), 'deadline')
            require(r['row'] is not None and not r['issued'], 'attempt_sequence')
            require(key in slots, 'slots')
            require(e['upstream'] == f'c{key[0]}r{key[1]}a{r["number"]}', 'correlation')
            r['issued'] = True
        elif op == 'connected':
            require(r['issued'] and r['result'] is None, 'structure')
            r['connected'] = True
        elif op == 'result':
            result = e['kind']
            require(result in RESULTS and r['row'] is not None and r['result'] is None, 'terminal')
            require(r['issued'] or result in ('deadline', 'canceled'), 'structure')
            if t >= r['deadline']:
                require(result in ('deadline', 'canceled', 'unknown'), 'deadline')
            if result == 'unknown':
                require(not r['safe'] and r['connected'], 'retry_safety')
            if result in ('ok', 'application'):
                require(e['upstream'] == f'c{key[0]}r{key[1]}a{r["number"]}', 'correlation')
                require(not r['cancel'], 'cancellation')
                require(t < r['deadline'], 'deadline')
            if result == 'deadline':
                require(t >= r['deadline'], 'deadline')
            if result == 'canceled':
                require(r['cancel'] is not None, 'cancellation')
            if not r['safe'] and r['connected'] and result in RETRY:
                require(False, 'retry_safety')
            r['result'] = result
            status = {'refused':'transport','reset':'transport','abort':'transport','broken_pipe':'transport',
                      'net_timeout':'transport','eof':'transport','wrong_id':'protocol','grant':'not_granted',
                      'application':'application_error','unknown':'unknown_outcome'}.get(result, result)
            rows[r['row']]['status'] = status
        elif op == 'release_slot':
            require(key in slots and r['row'] is not None, 'slots')
            require(r['result'] is not None or r['cancel'] is not None, 'terminal')
            if r['result'] is None:
                rows[r['row']]['status'] = 'canceled'
            slots.remove(key)
            r['row'] = None
        elif op == 'backoff':
            require(r['safe'] and r['result'] in RETRY and r['number'] < 3, 'retry_safety')
            require(key not in slots, 'slots')
            require(not r['cancel'] and not r['terminal'], 'cancellation')
            require(e['until'] == t + (10 if r['number'] == 1 else 20) and r['wake'] is None, 'backoff')
            r['wake'] = e['until']
        elif op == 'wake':
            require(not r['cancel'], 'cancellation')
            require(t < r['deadline'], 'deadline')
            require(r['wake'] is not None and t >= r['wake'], 'backoff')
            r.update(wake=None, ready=True)
        elif op == 'cancel':
            require(e['cause'] in ('deadline', 'parent'), 'cancellation')
            if e['cause'] == 'deadline':
                require(r['deadline'] is not None and t >= r['deadline'], 'deadline')
            r['cancel'] = r['cancel'] or e['cause']
            r['wake'] = None
        elif op == 'terminal':
            require(not r['terminal'] and key not in slots, 'terminal')
            reason = e['reason']
            require(r['cancel'] is not None or t < r['deadline'], 'deadline')
            if r['cancel']:
                expected = ('deadline' if r['safe'] else 'unknown_outcome') if r['cancel'] == 'deadline' else 'canceled'
            else:
                require(r['result'] is not None, 'terminal')
                require(not (r['safe'] and r['result'] in RETRY and r['number'] < 3), 'retry_safety')
                expected = rows[next(i for i in range(len(rows)-1,-1,-1) if (rows[i]['client'],rows[i]['request']) == key)]['status']
            require(reason == expected, 'terminal')
            require(e['id'] == r['id'], 'correlation')
            r['terminal'] = True
        elif op == 'release_request':
            require(r['credit'] and r['terminal'] and key not in slots, 'request_credit')
            require(e['via'] in ('delivered', 'drop'), 'request_credit')
            if e['via'] == 'drop':
                require(r['cancel'] or key[0] in disconnected, 'request_credit')
            r['credit'] = False
        else:
            require(False, 'structure')
    require(joined, 'join')
    ordered = sorted(rows, key=lambda x: (x['client'], x['request'], x['attempt']))
    require(v['report'] == ordered, 'report')
    output = '{"listening":true}\n' + ''.join(json.dumps(row, separators=(',', ':')) + '\n' for row in ordered) + '{"clients":1,"expired":true}\n'
    require(v['replay']['expected'] == dict(stdout=output, stderr='', exit=1), 'report')
    # Admission correctness never uses these bytes as its authority.
    require(v['replay']['expected'] == v['replay']['actual'], 'replay')
    return covered

def check_catalog(data):
    if set(data['invariants']) != INVARIANTS:
        raise ValueError('invariant catalog mismatch')
    positive, negative, exercised, rejected = 0, 0, set(), set()
    names = set()
    for v in data['vectors']:
        if v['name'] in names:
            raise ValueError('duplicate vector name')
        names.add(v['name'])
        try:
            exercised |= check(v)
            actual = 'accept'
        except Violation as error:
            actual = str(error)
        if actual != v['expected']:
            raise ValueError(f"{v['name']}: expected {v['expected']}, got {actual}")
        if actual == 'accept':
            positive += 1
        else:
            negative += 1
            rejected.add(actual)
    if exercised != INVARIANTS or rejected != INVARIANTS:
        raise ValueError(f'missing positive checks {INVARIANTS-exercised}; negative coverage {INVARIANTS-rejected}')
    return dict(positive=positive, negative=negative, invariants=len(INVARIANTS))


def judge_recording(view, recorded_output):
    """Judge the Rust lineage consumer's result, then the admission invariants.

    `view` is an internal adapter input: config/report/replay plus `lineage`
    returned by lineage.rs::consume after Trace::analyze. It is not a decoder
    entry point for arbitrary JSON sidecars or a claim of emitter coverage.
    The live tape reader still supplies None until its mappings are admitted.
    """
    verdict = dict(semantic_verdict="unproven: witness missing",
                   coverage=dict(admission="unproven", request_credits="unproven",
                                 upstream_credits="unproven"),
                   recorded_output=recorded_output)
    if not isinstance(view, dict) or not isinstance(view.get('lineage'), dict):
        return verdict
    lineage = view['lineage']
    verdict['missing_evidence'] = lineage.get('missing', ['lineage coverage missing'])
    verdict['witnessed'] = lineage.get('witnessed', [])
    coverage = lineage.get('coverage', {})
    if (lineage.get('complete') is not True or lineage.get('missing') != []
            or not isinstance(coverage, dict)
            or any(coverage.get(key) != 'witnessed' for key in verdict['coverage'])):
        return verdict
    try:
        replay = view['replay']
        # Report/replay remain separately checked facts, never ownership evidence.
        if replay['expected'] != recorded_output:
            raise Violation('report')
        check(dict(config=view['config'], events=lineage['events'],
                   report=view['report'], replay=replay))
    except Violation as error:
        verdict['semantic_verdict'] = 'fail: ' + str(error)
    except (KeyError, TypeError, ValueError, IndexError):
        verdict['missing_evidence'] = ['admission identity/timing/report mapping missing']
        return verdict
    else:
        verdict['semantic_verdict'] = 'pass'
    verdict['coverage'] = coverage
    return verdict


def main(path):
    result = check_catalog(json.loads(Path(path).read_text()))
    print(f"PASS: {result['positive']} positive, {result['negative']} negative; "
          f"{result['invariants']} named invariants covered positively and negatively")

if __name__ == '__main__':
    try:
        main(sys.argv[1])
    except (ValueError, KeyError, TypeError, IndexError) as error:
        print(f'FAIL: {error}', file=sys.stderr)
        sys.exit(1)
