#!/usr/bin/env python3
"""Generate the std@1/toml decoder conformance cases from toml-test.

Every case listed in toml-test's tests/files-toml-1.1.0 becomes one call in
conformance/SEM/SEM-3-toml-test-<group>-<n>-run.hir: a valid case must decode to
its tagged JSON and survive a stringify/parse round trip, an invalid case must
be rejected (packages/fixtures/toml_test@1/harness.hir). Expected floats are
passed as their binary64 bit patterns. Inputs that are not UTF-8 are written as
Bytes literals: they never become the String toml.parse takes. Each group is
split into programs whose estimated interpreter cost stays well under the
conformance runner's step budget.

Usage: tools/generate-toml-test.py <toml-test checkout> [--hirsel <binary>]
The checkout must be at COMMIT. With --hirsel the outputs are formatted
canonically with `hirsel fmt`.
"""

import argparse
import json
import struct
import subprocess
from pathlib import Path

COMMIT = 'ff49d109861c1ad25af53f687f2aef19ab650600'
ROOT = Path(__file__).resolve().parents[1]
OUT = ROOT / 'conformance/SEM'
HARNESS = 'packages/fixtures/toml_test@1/harness.hir'

# Estimated interpreter steps of one check: per call, per input byte and per
# float literal (decoding parses it once; a round trip formats it and parses it
# twice). One program spends at most about half the conformance runner's budget
# of 1,000,000 steps; a valid case over it decodes and round-trips in separate
# programs.
INVALID_STEPS = (5000, 200)
DECODE_STEPS = (6000, 200, 25000)
ROUND_TRIP_STEPS = (6000, 200, 90000)
STEPS_PER_PROGRAM = 550000

BIDI = {0x061C, 0x200E, 0x200F, *range(0x202A, 0x202F), *range(0x2066, 0x206A)}


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
        elif cp < 0x20 or cp == 0x7F or cp in BIDI:
            out.append('\\u{%x}' % cp)
        else:
            out.append(ch)
    out.append('"')
    return ''.join(out)


def bytes_literal(data):
    out = ['b"']
    for b in data:
        if b == 0x5C:
            out.append('\\\\')
        elif b == 0x22:
            out.append('\\"')
        elif b == 0x0A:
            out.append('\\n')
        elif b == 0x0D:
            out.append('\\r')
        elif b == 0x09:
            out.append('\\t')
        elif b < 0x20 or b >= 0x7F:
            out.append('\\x%02x' % b)
        else:
            out.append(chr(b))
    out.append('"')
    return ''.join(out)


def group_of(case):
    parts = case.split('/')
    sub = parts[1].removesuffix('-1.0.0') if len(parts) > 2 else 'misc'
    return f'{parts[0]}-{sub}'


def expectation(tests, name):
    """The case's tagged JSON, compact, with each float's value as the decimal
    binary64 bit pattern (Python's float() rounds correctly) or nan."""
    def bits(node):
        if isinstance(node, list):
            return [bits(item) for item in node]
        if set(node) == {'type', 'value'} and isinstance(node['value'], str):
            if node['type'] == 'float' and not node['value'].lstrip('+-').startswith('nan'):
                node = {'type': 'float', 'value': str(struct.unpack('<Q', struct.pack('<d', float(node['value'])))[0])}
            elif node['type'] == 'float':
                node = {'type': 'float', 'value': 'nan'}
            return node
        return {key: bits(item) for key, item in node.items()}
    expected = json.loads((tests / (name + '.json')).read_text(encoding='utf-8'))
    return json.dumps(bits(expected), ensure_ascii=False, separators=(',', ':'))


def checks(tests, case):
    """(estimated steps, call) for each check of one case."""
    name = case[:-len('.toml')]
    data = (tests / case).read_bytes()
    if case.startswith('valid/'):
        text = string_literal(data.decode('utf-8'))
        expected = expectation(tests, name)
        floats = expected.count('{"type":"float"')
        size = len(data) + len(expected.encode('utf-8'))
        decode = DECODE_STEPS[0] + DECODE_STEPS[1] * size + DECODE_STEPS[2] * floats
        round_trip = ROUND_TRIP_STEPS[0] + ROUND_TRIP_STEPS[1] * size + ROUND_TRIP_STEPS[2] * floats
        if decode + round_trip <= STEPS_PER_PROGRAM:
            return [(decode + round_trip,
                     f'  failed += harness.valid(s, {string_literal(name)}, {text}, {string_literal(expected)})')]
        return [(decode, f'  failed += harness.decodes(s, {string_literal(name)}, {text}, {string_literal(expected)})'),
                (round_trip, f'  failed += harness.round_trips(s, {string_literal(name)}, {text})')]
    steps = INVALID_STEPS[0] + INVALID_STEPS[1] * len(data)
    try:
        text = data.decode('utf-8')
    except UnicodeDecodeError:
        return [(steps, f'  failed += harness.invalid_bytes(s, {string_literal(name)}, {bytes_literal(data)})')]
    return [(steps, f'  failed += harness.invalid(s, {string_literal(name)}, {string_literal(text)})')]


def programs(tests, cases):
    """The checks of a group's cases, split into programs within the step budget."""
    out = [[]]
    spent = 0
    for case in cases:
        for steps, call in checks(tests, case):
            if out[-1] and spent + steps > STEPS_PER_PROGRAM:
                out.append([])
                spent = 0
            out[-1].append(call)
            spent += steps
    return out


def program(group, lines):
    return '\n'.join([
        '// rule: SEM-3',
        '// expect: run',
        f'// Generated by tools/generate-toml-test.py from toml-test {COMMIT[:12]}',
        f'// tests/files-toml-1.1.0, group {group}.',
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


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('toml_test', type=Path)
    parser.add_argument('--hirsel', type=Path)
    args = parser.parse_args()
    head = subprocess.run(['git', '-C', str(args.toml_test), 'rev-parse', 'HEAD'],
                          check=True, capture_output=True, text=True).stdout.strip()
    if head != COMMIT:
        raise SystemExit(f'toml-test is at {head}, expected {COMMIT}')
    tests = args.toml_test / 'tests'
    listed = [line.strip() for line in (tests / 'files-toml-1.1.0').read_text().splitlines()]
    cases = [c for c in listed if c.endswith('.toml')]
    groups = {}
    for case in cases:
        groups.setdefault(group_of(case), []).append(case)
    for old in OUT.glob('SEM-3-toml-test-*'):
        old.unlink()
    written = []
    for group, members in sorted(groups.items()):
        for n, lines in enumerate(programs(tests, members), 1):
            stem = f'SEM-3-toml-test-{group}-{n}-run'
            path = OUT / f'{stem}.hir'
            path.write_text(program(group, lines), encoding='utf-8')
            (OUT / f'{stem}.hir.json').write_text(manifest(stem), encoding='utf-8')
            written.append(path)
    if args.hirsel:
        for path in written:
            subprocess.run([str(args.hirsel), 'fmt', str(path)], check=True)
    print(f'{len(cases)} cases in {len(written)} files')


if __name__ == '__main__':
    main()
