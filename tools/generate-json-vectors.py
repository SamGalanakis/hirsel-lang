#!/usr/bin/env python3
"""Import every JSONTestSuite parsing, CBOR Appendix A and kawanet vector.

Usage: tools/generate-json-vectors.py JSONTestSuite cbor-test-vectors msgpack-test-suite [--hirsel CLI]
Checkouts must have the pinned revisions below. Generated .hir programs and
launch manifests are checked in, following generate-yaml-test.py's structure.
No skip list: every input is either decoded/re-encoded or explicitly rejected.
See docs/json-domain-vectors.md for the policies, counts and source citations.
"""
import argparse
import json
import re
import struct
import subprocess
from pathlib import Path

from importlib.util import module_from_spec, spec_from_file_location

ROOT = Path(__file__).resolve().parents[1]
OUT = ROOT / 'conformance/SEM'
HARNESS = 'packages/fixtures/json_vectors@1/harness.hir'
COMMITS = ('1ef36fa01286573e846ac449e8683f8833c5b26a',
           'aba89b653e484bc8573c22f3ff35641d79dfd8c1',
           'e04f6edeaae589c768d6b70fcce80aa786b7800e')
# Share the established Hirsel Unicode literal escaping with the YAML importer.
spec = spec_from_file_location('yaml_generator', ROOT / 'tools/generate-yaml-test.py')
yaml = module_from_spec(spec)
spec.loader.exec_module(yaml)
literal = yaml.string_literal


def compact(value):
    return json.dumps(value, ensure_ascii=False, separators=(',', ':'))


def blob(data):
    return 'b"' + ''.join(f'\\x{b:02x}' for b in data) + '"'


def call(method, name, *args):
    return f'  failed += harness.{method}(s, {literal(name)}, ' + ', '.join(args) + ')'


def json_cases(suite):
    counts = dict(y=0, n=0, i=0, utf8_reject=0)
    for path in sorted((suite / 'test_parsing').glob('*.json')):
        data = path.read_bytes()
        try:
            text = data.decode('utf-8')
        except UnicodeDecodeError:
            counts['utf8_reject'] += 1
            yield call('invalid_utf8', path.name + ':1', blob(data)), False
            continue
        kind = path.name[0]
        counts[kind] += 1
        outcome = 0 if kind == 'y' or path.name.startswith('i_number_') else 1
        if path.name.startswith('y_object_duplicated_key'):
            outcome = 2
        yield call('parses', path.name + ':1', literal(text), str(outcome)), False
    assert counts == dict(y=95, n=176, i=22, utf8_reject=25), counts
    print('JSONTestSuite:', counts, '(318 inputs)')


def cbor_cases(suite):
    source = (suite / 'appendix_a.json').read_text()
    vectors = json.loads(source)
    line_numbers = [i for i, line in enumerate(source.splitlines(), 1) if '"hex":' in line]
    assert len(line_numbers) == len(vectors)
    accepted = rejected = 0
    for i, vector in enumerate(vectors):
        data = bytes.fromhex(vector['hex'])
        name = f'appendix_a.json:{line_numbers[i]} [{i}] {vector["hex"]}'
        if 'decoded' not in vector or data[0] >> 5 in (2, 6):
            rejected += 1
            # RFC 7049's simple(24) example is reserved/malformed under
            # RFC 8949 section 3.3; retain the decoder's Syntax result.
            method = 'syntax' if data == bytes.fromhex('f818') else 'unsupported'
            yield call(method, name, 'true', blob(data)), False
            continue
        accepted += 1
        expected = compact(vector['decoded'])
        canonical = data
        if not vector['roundtrip']:
            # Appendix A lacks definite counterparts for these two rows.
            counterparts = {'7f657374726561646d696e67ff': '6973747265616d696e67',
                            'bf6346756ef563416d7421ff': 'a263416d74216346756ef5'}
            h = counterparts.get(vector['hex'])
            if h is None:
                h = next(v['hex'] for v in vectors if v['roundtrip']
                         and 'decoded' in v and compact(v['decoded']) == expected)
            canonical = bytes.fromhex(h)
        yield call('wire', name, 'true', blob(data), literal(expected), blob(canonical)), isinstance(vector['decoded'], float)
    assert (accepted, rejected) == (57, 25), (accepted, rejected)
    print(f'CBOR: {accepted} accept, {rejected} reject (82 inputs, including bignum tags)')


def msgpack_cases(suite):
    vectors = json.loads((suite / 'dist/msgpack-test-suite.json').read_text())
    accepted = rejected = encodings = rejects = 0
    for filename, cases in vectors.items():
        source_lines = (suite / 'src' / filename).read_text().splitlines()
        line_numbers = [i for i, line in enumerate(source_lines, 1) if line.startswith('- ')]
        assert len(line_numbers) == len(cases)
        source_encodings = re.findall(r'^    - "([0-9a-f-]+)"', '\n'.join(source_lines), re.M)
        assert source_encodings == [h for case in cases for h in case['msgpack']]
        for i, case in enumerate(cases):
            name = f'src/{filename}:{line_numbers[i]}'
            wires = [bytes.fromhex(h.replace('-', '')) for h in case['msgpack']]
            if any(k in case for k in ('binary', 'ext', 'timestamp')):
                rejected += 1
                for wire in wires:
                    rejects += 1
                    yield call('unsupported', name, 'false', blob(wire)), False
                continue
            accepted += 1
            if 'bignum' in case:
                value = int(case['bignum'])
            else:
                value = next(case[k] for k in ('nil', 'bool', 'number', 'string', 'array', 'map') if k in case)
            canonical = min(wires, key=len)
            for wire in wires:
                encodings += 1
                expected = value
                preferred = canonical
                if wire[0] in (0xca, 0xcb):
                    expected = struct.unpack('>f' if wire[0] == 0xca else '>d', wire[1:])[0]
                    # These suite alternatives encode exactly the same number.
                    assert expected == value
                    preferred = b'\xcb' + struct.pack('>d', expected)
                elif type(value) is int:
                    # Preserve integer type even when a float alternative is
                    # shorter. Prefer unsigned over signed for positive ties.
                    integers = [w for w in wires if w[0] not in (0xca, 0xcb)]
                    if value >= 0:
                        integers = [w for w in integers if w[0] <= 0x7f or 0xcc <= w[0] <= 0xcf]
                    preferred = min(integers, key=len)
                yield call('wire', name, 'false', blob(wire), literal(compact(expected)), blob(preferred)), isinstance(expected, float)
    assert (accepted, encodings, rejected, rejects) == (56, 194, 29, 39)
    print(f'MessagePack: {accepted} JSON cases/{encodings} encodings; {rejected} unsupported cases/{rejects} encodings')
    # rmp/tests/func: signed positive maxima, max f32, nonfinite, UTF-8,
    # truncated str headers/payloads and empty map16/map32 (audit item 3).
    for h, value, canonical in (
        ('d07f', 127, '7f'), ('d17fff', 32767, 'cd7fff'),
        ('d27fffffff', 2147483647, 'ce7fffffff'),
        ('d37fffffffffffffff', 9223372036854775807, 'cf7fffffffffffffff'),
        ('ca7f7fffff', struct.unpack('>f', bytes.fromhex('7f7fffff'))[0], 'cb47efffffe0000000'),
        ('de0000', {}, '80'), ('df00000000', {}, '80'),
        ('d900', '', 'a0'),
    ):
        yield call('wire', f'rmp edge {h}', 'false', blob(bytes.fromhex(h)), literal(compact(value)), blob(bytes.fromhex(canonical))), isinstance(value, float)
    for h in ('ca7f800000', 'caff800000', 'cb7ff0000000000000', 'cbfff0000000000000'):
        yield call('unsupported', f'rmp nonfinite {h}', 'false', blob(bytes.fromhex(h))), False
    for h, kind in (('a2c328', 1), ('d9ff', 0), ('bf', 0)):
        yield call('edge', f'rmp invalid {h}', blob(bytes.fromhex(h)), str(kind)), False


def write_programs(label, cases, cli):
    for old in OUT.glob(f'SEM-3-{label}-import-*-run.hir*'):
        old.unlink()
    groups, pending = [], []
    for line, expensive in cases:
        if expensive and pending:
            groups.append(pending)
            pending = []
        pending.append(line)
        if expensive or len(pending) == 8:
            groups.append(pending)
            pending = []
    if pending:
        groups.append(pending)
    for i, lines in enumerate(groups, 1):
        stem = f'SEM-3-{label}-import-{i:03}-run'
        path = OUT / f'{stem}.hir'
        path.write_text('// rule: SEM-3\n// expect: run\n'
            '// Generated by tools/generate-json-vectors.py; see docs/json-domain-vectors.md.\n'
            'fn grant_error(take e: GrantError) -> () {}\n\n'
            'pub fn main(rt: Io) -> Result[(), ()] {\n'
            '  let s = io.stdout(rt) ? grant_error\n  let mut failed: u64 = 0\n'
            + '\n'.join(lines) + '\n  failure.check(failed == 0, TextInvariant())\n  Ok(())\n}\n')
        manifest = {'schema': 1, 'package': {'name': 'application', 'version': '1'},
            'entry': {'module': [], 'name': 'main'},
            'sources': [{'path': path.name, 'module': []}, {'path': '../../' + HARNESS, 'module': ['harness']}],
            'dependencies': [{'manifest': '../../stdlib/std@1/hirsel.json', 'package': {'name': 'std', 'version': '1'}, 'expose': []}]}
        path.with_suffix('.hir.json').write_text(json.dumps(manifest, indent=2) + '\n')
        if cli:
            subprocess.run([str(cli), 'fmt', str(path)], check=True)
    print(f'{label}: {len(groups)} corpus programs')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ('json_suite', 'cbor_suite', 'msgpack_suite'):
        parser.add_argument(name, type=Path)
    parser.add_argument('--hirsel', type=Path)
    args = parser.parse_args()
    suites = (args.json_suite, args.cbor_suite, args.msgpack_suite)
    for suite, commit in zip(suites, COMMITS):
        head = subprocess.check_output(['git', '-C', str(suite), 'rev-parse', 'HEAD'], text=True).strip()
        if head != commit:
            parser.error(f'{suite}: expected {commit}, got {head}')
    for label, cases in zip(('json', 'cbor', 'msgpack'), (json_cases(suites[0]), cbor_cases(suites[1]), msgpack_cases(suites[2]))):
        write_programs(label, cases, args.hirsel)
    # Keep registration sorted until passing.txt is retired.
    passing = ROOT / 'conformance/passing.txt'
    entries = {s for s in passing.read_text().splitlines() if '-import-' not in s or not any(f'SEM-3-{x}-import-' in s for x in ('json', 'cbor', 'msgpack'))}
    entries.update(str(p.relative_to(ROOT)) for label in ('json', 'cbor', 'msgpack') for p in OUT.glob(f'SEM-3-{label}-import-*-run.hir'))
    passing.write_text('\n'.join(sorted(entries)) + '\n')


if __name__ == '__main__':
    main()
