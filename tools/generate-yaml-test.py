#!/usr/bin/env python3
"""Generate the std@1/yaml parser conformance cases from yaml-test-suite.

Every case of yaml-test-suite's data branch (a directory holding in.yaml) becomes
one call in conformance/SEM/SEM-3-yaml-test-<n>-run.hir
(packages/fixtures/yaml_test@1/harness.hir):

- a case with an `error` file must be rejected;
- a collection (or an alias of one) as a mapping key must fail with ComplexKey,
  and repeated key text with DuplicateKey;
- non-core tags are retained as Tagged values, checked against the event tree
  (including resolved handles and aliases), then round-tripped;
- any other case with in.json must parse to its JSON documents, each of which
  must survive a stringify/parse round trip, and one without must match the
  exact values of its resolved event tree.

Cases are split into programs of at most 19 times the estimated cost that one
program of the default 1M-step budget may spend; each program's `// steps:`
header raises its budget to match, since a program costs about 6 s of gate time
before its first check.

Usage: tools/generate-yaml-test.py <yaml-test-suite checkout> [--hirsel <binary>]
The checkout must be the data branch at COMMIT. With --hirsel the outputs are
formatted canonically with `hirsel fmt`.
"""

import argparse
import json
import re
import subprocess
from pathlib import Path

COMMIT = '6e6c296ae9c9d2d5c4134b4b64d01b29ac19ff6f'
ROOT = Path(__file__).resolve().parents[1]
OUT = ROOT / 'conformance/SEM'
HARNESS = 'packages/fixtures/yaml_test@1/harness.hir'
# The complete invoice round trip exceeds the corpus's 1M interpreter steps.
# Its parse/tree check stays in the corpus; a Rust driver test runs this fixture.
RUST_ROUND_TRIPS = {'UGM3': 'yaml-tagged-invoice'}

# Estimated interpreter steps, measured with `hirsel run --max-steps` and rounded
# up: a parse per YAML byte and per float; decoding adds a JSON parse per JSON byte
# and per float; a round trip of one document adds a stringify and a parse per
# byte and per float of that document (estimated as its share of the input). One
# 700k estimated steps fit the runner's default 1,000,000; a program spends at
# most 19 such units and raises its `// steps:` budget to 1M a unit.
CALL_STEPS = 10000
PARSE_STEPS = (1100, 50000)
JSON_STEPS = (400, 30000)
ROUND_TRIP_STEPS = (1600, 110000)
STEPS_UNIT = 700000
STEPS_PER_PROGRAM = 19 * STEPS_UNIT

CORE = {f'tag:yaml.org,2002:{name}' for name in ('str', 'int', 'float', 'bool', 'null', 'seq', 'map')}
CORE.add('!')

ESCAPED = {0x061C, 0x200E, 0x200F, *range(0x202A, 0x202F), *range(0x2066, 0x206A),
           0x85, 0x2028, 0x2029, 0xFEFF}


def string_literal(text):
    out = ['"']
    for ch in text:
        cp = ord(ch)
        if ch == '\\':
            out.append('\\\\')
        elif ch == '"':
            out.append('\\"')
        elif ch == '\n':
            out.append('\\n')
        elif ch == '\r':
            out.append('\\r')
        elif ch == '\t':
            out.append('\\t')
        elif cp < 0x20 or 0x7F <= cp < 0xA0 or cp in ESCAPED:
            out.append('\\u{%x}' % cp)
        else:
            out.append(ch)
    out.append('"')
    return ''.join(out)


def json_documents(text):
    """Each JSON document of a concatenated in.json, compact."""
    decoder = json.JSONDecoder()
    docs = []
    at = 0
    while True:
        while at < len(text) and text[at].isspace():
            at += 1
        if at == len(text):
            return docs
        value, at = decoder.raw_decode(text, at)
        docs.append(json.dumps(value, ensure_ascii=False, separators=(',', ':')))


def floats_in(value):
    if isinstance(value, float):
        return 1
    if isinstance(value, list):
        return sum(floats_in(v) for v in value)
    if isinstance(value, dict):
        return sum(floats_in(v) for v in value.values())
    return 0


def outside_model(events):
    """'complex_key' or 'duplicate_key' for the first node the composer
    cannot represent, or None. Mirrors compose_node: a key position checks the node's
    kind before its tag, and keys compare by their text."""
    # Each open collection: [kind, children seen, key texts]; a mapping's even
    # children are keys.
    stack = []
    collections = set()
    scalars = {}
    for line in events.splitlines():
        parts = line.split(' ')
        head = parts[0]
        if head in ('+STR', '-STR', '+DOC', '-DOC'):
            continue
        if head in ('-SEQ', '-MAP'):
            stack.pop()
            if stack:
                stack[-1][1] += 1
            continue
        key = bool(stack) and stack[-1][0] == 'map' and stack[-1][1] % 2 == 0
        anchor = next((p[1:] for p in parts[1:] if p.startswith('&')), None)
        if head == '=ALI':
            if key and parts[1][1:] in collections:
                return 'complex_key'
            if key and scalars.get(parts[1][1:]) in stack[-1][2]:
                return 'duplicate_key'
            if key:
                stack[-1][2].add(scalars.get(parts[1][1:]))
            if stack:
                stack[-1][1] += 1
            continue
        if head in ('+SEQ', '+MAP'):
            if key:
                return 'complex_key'
            if anchor is not None:
                collections.add(anchor)
            stack.append(['seq' if head == '+SEQ' else 'map', 0, set()])
            continue
        # =VAL
        # =VAL [&anchor] [<tag>] <style><content>
        content = line[len('=VAL '):]
        while content[0] in '&<':
            content = content.split(' ', 1)[1]
        content = content[1:]
        if anchor is not None:
            collections.discard(anchor)
            scalars[anchor] = content
        if key and content in stack[-1][2]:
            return 'duplicate_key'
        if key:
            stack[-1][2].add(content)
        if stack:
            stack[-1][1] += 1
    return None


def event_values(events):
    """Expected Value constructors from the suite's resolved event tree.

    Used for tagged cases, whose tags JSON cannot represent. Scalars here use the
    1.2 core schema; the suite's YAML-1.1 integer-looking floats remain Float.
    """
    lines = iter(events.splitlines())
    anchors = {}

    def node(line, key=False):
        head, _, rest = line.partition(' ')
        if head == '=ALI':
            value, text = anchors[rest[1:]]
            return (string_literal(text) if key else value), text
        anchor = None
        tag = None
        while rest.startswith(('&', '<', '{} ', '[] ')):
            prop, _, rest = rest.partition(' ')
            if prop.startswith('&'):
                anchor = prop[1:]
            elif prop.startswith('<'):
                tag = prop[1:-1]
        if head in ('+MAP', '+SEQ'):
            children = []
            while True:
                child = next(lines)
                if child == ('-MAP' if head == '+MAP' else '-SEQ'):
                    break
                value, _ = node(child, head == '+MAP' and len(children) % 2 == 0)
                children.append(value)
            if head == '+MAP':
                pairs = [f'({k}, {v})' for k, v in zip(children[::2], children[1::2])]
                value = 'harness.object([' + ', '.join(pairs) + '])'
            else:
                value = 'Array([' + ', '.join(children) + '])'
            text = None
        else:
            style, text = rest[0], rest[1:]
            text = re.sub(r'\\([nrt\\])', lambda m: {'n': '\n', 'r': '\r', 't': '\t', '\\': '\\'}[m[1]], text)
            if tag not in (None, *CORE) or style != ':' or tag in ('!', 'tag:yaml.org,2002:str'):
                value = 'Text(' + string_literal(text) + ')'
            elif text in ('', '~', 'null', 'Null', 'NULL'):
                value = 'Null()'
            elif text in ('true', 'True', 'TRUE', 'false', 'False', 'FALSE'):
                value = 'Bool(' + text.lower() + ')'
            elif re.fullmatch(r'[-+]?[0-9]+|0x[0-9a-fA-F]+|0o[0-7]+', text):
                base = 16 if text.startswith('0x') else 8 if text.startswith('0o') else 10
                value = f'Integer({int(text, base)})'
            elif re.fullmatch(r'[-+]?(?:[0-9]+(?:\.[0-9]*)?|\.[0-9]+)(?:[eE][-+]?[0-9]+)?', text):
                value = f'Float({float(text)!r})'
            else:
                value = 'Text(' + string_literal(text) + ')'
        if tag is not None and tag not in CORE:
            value = f'Tagged(tag: {string_literal(tag)}, value: box {value})'
        if anchor is not None:
            anchors[anchor] = value, text
        return (string_literal(text) if key else value), text

    documents = []
    for line in lines:
        if line.startswith('+DOC'):
            anchors.clear()
            documents.append(node(next(lines))[0])
    return documents


def checks(case):
    """(estimated steps, call) for each check of one case directory."""
    name = case.parent.name + '/' + case.name if case.parent != SUITE else case.name
    data = (case / 'in.yaml').read_bytes()
    text = string_literal(data.decode('utf-8'))
    label = string_literal(name)
    steps = CALL_STEPS + PARSE_STEPS[0] * len(data)
    if (case / 'error').exists():
        return [(steps, f'  failed += harness.invalid(s, {label}, {text})')]
    events = (case / 'test.event').read_text(encoding='utf-8')
    kind = outside_model(events)
    if kind is not None:
        return [(steps, f'  failed += harness.{kind}(s, {label}, {text})')]
    if any(p.startswith('<') and p[1:-1] not in CORE
           for line in events.splitlines() for p in line.split(' ')):
        values = event_values(events)
        # Decode and round-trip separately: binary and invoice examples are large.
        # Round-trip the independent event value, avoiding a second input parse.
        expected = ', '.join(values)
        decoded = [(steps, f'  failed += harness.values(s, {label}, {text}, [{expected}])')]
        if name in RUST_ROUND_TRIPS:
            return decoded
        return decoded + [
            (CALL_STEPS + ROUND_TRIP_STEPS[0] * len(data) // len(values),
             f'  failed += harness.round_trip(s, {label}, {value})')
            for value in values]
    if (case / 'in.json').exists():
        docs = json_documents((case / 'in.json').read_text(encoding='utf-8'))
        size = sum(len(d.encode('utf-8')) for d in docs)
        floats = [floats_in(json.loads(d)) for d in docs]
        parse = steps + PARSE_STEPS[1] * sum(floats)
        decode = parse + JSON_STEPS[0] * size + JSON_STEPS[1] * sum(floats)
        trips = [ROUND_TRIP_STEPS[0] * len(data) // len(docs) + ROUND_TRIP_STEPS[1] * f for f in floats]
        expected = ', '.join(string_literal(d) for d in docs)
        if decode + sum(trips) <= STEPS_PER_PROGRAM:
            return [(decode + sum(trips), f'  failed += harness.valid(s, {label}, {text}, [{expected}])')]
        return [(decode, f'  failed += harness.decodes(s, {label}, {text}, [{expected}])')] + [
            (parse + trip, f'  failed += harness.round_trips(s, {label}, {text}, {i})')
            for i, trip in enumerate(trips)]
    expected = ', '.join(event_values(events))
    return [(steps, f'  failed += harness.values(s, {label}, {text}, [{expected}])')]


def programs(cases):
    out = [[]]
    spent = 0
    for case in cases:
        for steps, call in checks(case):
            if out[-1] and spent + steps > STEPS_PER_PROGRAM:
                out.append([])
                spent = 0
            out[-1].append((steps, call))
            spent += steps
    return [(-(-sum(s for s, _ in calls) // STEPS_UNIT) * 1000000, [c for _, c in calls]) for calls in out]


def program(lines, steps=None):
    return '\n'.join([
        '// rule: SEM-3',
        '// expect: run',
        *([f'// steps: {steps}', '// one program runs this batch of the suite, which outgrows the default budget'] if steps else []),
        f'// Generated by tools/generate-yaml-test.py from yaml-test-suite {COMMIT[:12]}.',
        'fn grant_error(take e: GrantError) -> () {}',
        '',
        'pub fn main(rt: Io) -> Result[(), ()] {',
        '  let s = io.stdout(rt) ? grant_error',
        '  let mut failed: u64 = 0',
        *lines,
        '  failure.check(failed == 0, TextInvariant())',
        '  Ok(())',
        '}',
        '',
    ])


def manifest(stem):
    return json.dumps({
        'schema': 1,
        'package': {'name': 'application', 'version': '1'},
        'entry': {'module': [], 'name': 'main'},
        'sources': [
            {'path': f'{stem}.hir', 'module': []},
            {'path': f'../../{HARNESS}', 'module': ['harness']},
        ],
        'dependencies': [{
            'manifest': '../../stdlib/std@1/hirsel.json',
            'package': {'name': 'std', 'version': '1'},
            'expose': [],
        }],
    }, indent=2) + '\n'


SUITE = None


def main():
    global SUITE
    parser = argparse.ArgumentParser()
    parser.add_argument('suite', type=Path)
    parser.add_argument('--hirsel', type=Path)
    args = parser.parse_args()
    head = subprocess.run(['git', '-C', str(args.suite), 'rev-parse', 'HEAD'],
                          check=True, capture_output=True, text=True).stdout.strip()
    if head != COMMIT:
        raise SystemExit(f'yaml-test-suite is at {head}, expected {COMMIT}')
    SUITE = args.suite.resolve()
    cases = sorted(p.parent for p in SUITE.glob('**/in.yaml') if '.git' not in p.parts)
    for old in OUT.glob('SEM-3-yaml-test-*'):
        old.unlink()
    written = []
    for n, (steps, lines) in enumerate(programs(cases), 1):
        stem = f'SEM-3-yaml-test-{n}-run'
        path = OUT / f'{stem}.hir'
        path.write_text(program(lines, steps), encoding='utf-8')
        (OUT / f'{stem}.hir.json').write_text(manifest(stem), encoding='utf-8')
        written.append(path)
    corpus_files = len(written)
    for name, stem in RUST_ROUND_TRIPS.items():
        events = (SUITE / name / 'test.event').read_text(encoding='utf-8')
        lines = [f'  failed += harness.round_trip(s, {string_literal(name)}, {value})'
                 for value in event_values(events)]
        fixture = ROOT / f'crates/hirsel-driver/tests/fixtures/{stem}.hir'
        fixture.write_text(program(lines), encoding='utf-8')
        launch = json.loads(manifest(stem))
        launch['sources'][1]['path'] = '../../../../' + HARNESS
        launch['dependencies'][0]['manifest'] = '../../../../stdlib/std@1/hirsel.json'
        fixture.with_suffix('.hir.json').write_text(json.dumps(launch, indent=2) + '\n')
        written.append(fixture)
    if args.hirsel:
        for path in written:
            subprocess.run([str(args.hirsel), 'fmt', str(path)], check=True)
    print(f'{len(cases)} cases in {corpus_files} corpus files and {len(RUST_ROUND_TRIPS)} Rust fixtures')


if __name__ == '__main__':
    main()
