#!/usr/bin/env python3
"""Generate the std@1/yaml parser conformance cases from yaml-test-suite.

Every case of yaml-test-suite's data branch (a directory holding in.yaml) becomes
one call in conformance/SEM/SEM-3-yaml-test-<n>-run.hir
(packages/fixtures/yaml_test@1/harness.hir):

- a case with an `error` file must be rejected;
- a case whose events hold a tag outside the YAML 1.2 core schema must fail with
  Tag, and one with a collection (or an alias of one) as a mapping key must fail
  with ComplexKey, and one whose mapping repeats a key's text must fail with
  DuplicateKey, whichever the composer meets first: the Value model has no place
  for any of them;
- any other case with in.json must parse to its JSON documents, each of which
  must survive a stringify/parse round trip, and one without must parse to its
  number of documents.

Cases are split into programs whose estimated interpreter cost stays well under
the conformance runner's step budget.

Usage: tools/generate-yaml-test.py <yaml-test-suite checkout> [--hirsel <binary>]
The checkout must be the data branch at COMMIT. With --hirsel the outputs are
formatted canonically with `hirsel fmt`.
"""

import argparse
import json
import subprocess
from pathlib import Path

COMMIT = '6e6c296ae9c9d2d5c4134b4b64d01b29ac19ff6f'
ROOT = Path(__file__).resolve().parents[1]
OUT = ROOT / 'conformance/SEM'
HARNESS = 'packages/fixtures/yaml_test@1/harness.hir'

# Estimated interpreter steps, measured with `hirsel run --max-steps` and rounded
# up: a parse per YAML byte and per float; decoding adds a JSON parse per JSON byte
# and per float; a round trip of one document adds a stringify and a parse per
# byte and per float of that document (estimated as its share of the input). One
# program spends at most about 70% of the runner's 1,000,000 steps; a valid case
# over that decodes and round-trips each document in separate programs.
CALL_STEPS = 10000
PARSE_STEPS = (1100, 50000)
JSON_STEPS = (400, 30000)
ROUND_TRIP_STEPS = (1600, 110000)
STEPS_PER_PROGRAM = 700000

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
    """'tagged', 'complex_key' or 'duplicate_key' for the first node the composer
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
        tag = next((p[1:-1] for p in parts[1:] if p.startswith('<')), None)
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
            if tag is not None and tag not in CORE:
                return 'tagged'
            if anchor is not None:
                collections.add(anchor)
            stack.append(['seq' if head == '+SEQ' else 'map', 0, set()])
            continue
        # =VAL
        if tag is not None and tag not in CORE:
            return 'tagged'
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
    count = sum(1 for line in events.splitlines() if line.startswith('+DOC'))
    return [(steps, f'  failed += harness.parses(s, {label}, {text}, {count})')]


def programs(cases):
    out = [[]]
    spent = 0
    for case in cases:
        for steps, call in checks(case):
            if out[-1] and spent + steps > STEPS_PER_PROGRAM:
                out.append([])
                spent = 0
            out[-1].append(call)
            spent += steps
    return out


def program(lines):
    return '\n'.join([
        '// rule: SEM-3',
        '// expect: run',
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
    for n, lines in enumerate(programs(cases), 1):
        stem = f'SEM-3-yaml-test-{n}-run'
        path = OUT / f'{stem}.hir'
        path.write_text(program(lines), encoding='utf-8')
        (OUT / f'{stem}.hir.json').write_text(manifest(stem), encoding='utf-8')
        written.append(path)
    if args.hirsel:
        for path in written:
            subprocess.run([str(args.hirsel), 'fmt', str(path)], check=True)
    print(f'{len(cases)} cases in {len(written)} files')


if __name__ == '__main__':
    main()
