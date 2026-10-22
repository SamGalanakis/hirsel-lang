#!/usr/bin/env python3
"""Port Go ebe2feef capture/replacement/flag vectors; expectations are upstream.

Usage: tools/generate-regex-captures-test.py /tmp/ref-go
Uses regexp/find_test.go, all_test.go, syntax/parse_test.go and the ordered
partial-match column of RE2's re2-search.txt (exec_test.go:70-200).
The exclusion report records every unsupported row rather than silently
accepting a smaller dialect. No Hirsel results are used as expectations.
"""
import importlib.util
import json
from pathlib import Path
import re
import subprocess
import sys
from collections import Counter

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('base', ROOT / 'tools/generate-regex-test.py')
base = importlib.util.module_from_spec(spec)
spec.loader.exec_module(base)
STRING = base.STRING
OUT = ROOT / 'conformance/SEM'
HEADER = '''// rule: SEM-3
// expect: run
// Go ebe2feef capture/flag/replacement vectors (BSD-3-Clause).
// RE2 data Copyright 2009 The RE2 Authors (BSD-3-Clause).
'''
CAPTURE = '''fn check(pattern: String, input: String, maximum: u64, expected: List[regex.Captures]) -> Result[(), ()] {
  let Ok(re) = regex.compile(pattern, 10000) else { return Error(()) }
  failure.check(regex.find_all_captures(re, input, maximum, 1000000) == Ok(expected), TextInvariant())
  if maximum != 0 {
    let first: Option[regex.Captures] = if list.len(expected) == 0 { None() } else { Some(copy expected[0]) }
    failure.check(regex.find_captures(re, input, 1000000) == Ok(first), TextInvariant())
  }
  Ok(())
}
'''
REPLACE = '''fn check(pattern: String, input: String, replacement: String, expected: String, literal: bool) -> Result[(), ()] {
  let Ok(re) = regex.compile(pattern, 10000) else { return Error(()) }
  let actual = if literal { regex.replace_all_literal(re, input, replacement, 1000000) }
    else { regex.replace_all(re, input, replacement, 1000000) }
  failure.check(actual == Ok(expected), TextInvariant())
  Ok(())
}
'''

def outside(pattern):
    # Teach the previous lane's lexical classifier only the newly ported forms.
    simplified = re.sub(r'\(\?(?:P?<[^>]*>|[imsU]*(?:-[imsU]+)?[:)])', '(', pattern)
    return base.outside(simplified)


def captures(numbers):
    return 'regex.Captures(spans: [' + ', '.join(
        'None()' if a < 0 else f'Some(regex.Span(start: {a}, end: {b}))'
        for a, b in zip(numbers[::2], numbers[1::2])) + '])'


def main():
    ref = Path(sys.argv[1])
    assert subprocess.check_output(['git', '-C', ref, 'rev-parse', 'HEAD'], text=True).strip() == base.COMMIT
    excluded = []
    counts = Counter()
    cases = {'captures': [], 're2': [], 'replacement': [], 'invalid': []}

    def include(file, line, pattern, text=b''):
        try:
            pattern, text = pattern.decode(), text.decode()
        except UnicodeDecodeError:
            excluded.append((file, line, 'invalid UTF-8 (Hirsel String is valid UTF-8)'))
            return None
        reason = outside(pattern)
        if reason:
            excluded.append((file, line, reason))
            return None
        return pattern, text

    source = (ref / 'src/regexp/find_test.go').read_text()
    start = source.index('var findTests = []FindTest{')
    end = source.index('\n}', start)
    row = re.compile(r'\{\s*(' + STRING + r')\s*,\s*(' + STRING + r')\s*,\s*(-?\d+)\s*,\s*(nil|build\([^)]*\))\s*,?\s*\}', re.S)
    for match in row.finditer(source, start, end):
        pat, text, maximum, spans = match.groups()
        line = source.count('\n', 0, match.start()) + 1
        pair = include('find_test.go', line, base.unquote(pat), base.unquote(text))
        if pair is None:
            continue
        pat, text = pair
        nums = list(map(int, re.findall(r'-?\d+', spans)))
        expected = []
        if nums:
            n, *nums = nums
            width = len(nums) // n
            expected = [captures(nums[i:i + width]) for i in range(0, len(nums), width)]
        maximum = 18446744073709551615 if int(maximum) < 0 else int(maximum)
        cases['captures'].append((f'find_test.go:{line}', f'  check({base.literal(pat)}, {base.literal(text)}, {maximum}, [{", ".join(expected)}])?'))

    strings, at, in_strings = [], 0, False
    for line, row in enumerate((ref / 'src/regexp/testdata/re2-search.txt').read_text().splitlines(), 1):
        if not row or row.startswith('#') or row[0].isupper(): continue
        if row == 'strings': strings, in_strings = [], True
        elif row == 'regexps': in_strings = False
        elif row.startswith('"'):
            value = base.unquote(row)
            if in_strings: strings.append(value)
            else: pattern, at = value, 0
        else:
            cells = row.split(';')
            text, at = strings[at], at + 1
            pair = include('re2-search.txt', line, pattern, text)
            if pair is None: continue
            pat, text = pair
            if '\\B' in pat and not text.isascii():
                excluded.append(('re2-search.txt', line, 'RE2 byte-interior boundary; skipped by Go exec_test.go:153-159 too'))
                continue
            spans = cells[1].split()
            expected = [] if spans == ['-'] else [captures([n for span in spans for n in ((-1, -1) if span == '-' else tuple(map(int, span.split('-'))))])]
            # RE2 rows describe the first ordered match, so cap enumeration at one.
            cases['re2'].append((f're2-search.txt:{line}', f'  check({base.literal(pat)}, {base.literal(text)}, 1, [{", ".join(expected)}])?'))

    source = (ref / 'src/regexp/all_test.go').read_text()
    row = re.compile(r'\{\s*(' + STRING + r')\s*,\s*(' + STRING + r')\s*,\s*(' + STRING + r')\s*,\s*(' + STRING + r')\s*\}', re.S)
    for table, literal in [('replaceTests', False), ('replaceLiteralTests', True)]:
        start = source.index('var ' + table + ' =')
        end = source.index('\n}', start)
        for match in row.finditer(source, start, end):
            pat, replacement, text, expected = map(base.unquote, match.groups())
            line = source.count('\n', 0, match.start()) + 1
            pair = include('all_test.go', line, pat, text)
            if pair is None: continue
            pat, text = pair
            cases['replacement'].append((f'all_test.go:{line}', f'  check({base.literal(pat)}, {base.literal(text)}, {base.literal(replacement.decode())}, {base.literal(expected.decode())}, {str(literal).lower()})?'))

    for line, value in base.expressions((ref / 'src/regexp/syntax/parse_test.go').read_text()):
        if b'(?P' not in value and b'(?<' not in value and b'(?i)' not in value: continue
        pair = include('syntax/parse_test.go', line, value)
        if pair is None: continue
        # Keep name/flag syntax rows, not Go's separately bounded count/depth tests.
        pat, _ = pair
        cases['invalid'].append((f'syntax/parse_test.go:{line}', f'  failure.check(match regex.compile({base.literal(pat)}, 10000) {{ Error(Syntax(_)) => true, _ => false, }}, TextInvariant())'))

    files = []
    for group, rows in cases.items():
        # Heavy folded classes and the nested replacement rows are split
        # without increasing the corpus's 1M interpreter-step budget.
        for batch in range(0, len(rows), 8):
            original = rows[batch:batch + 8]
            heavy = group == 're2' and any('acegikmoqsuwy' in call for _, call in original) or group == 'replacement' and batch == 64
            size = 2 if heavy else 8
            for part in range(0, len(original), size):
                suffix = '' if part == 0 else f'-part-{part // size + 1}'
                path = OUT / f'SEM-3-regex-captures-{group}-{batch // 8 + 1}{suffix}-run.hir'
                helper = CAPTURE if group in ('captures', 're2') else REPLACE if group == 'replacement' else ''
                content = HEADER + helper + '\npub fn main() -> Result[(), ()] {\n'
                for origin, call in original[part:part + size]:
                    content += f'  // Go {origin}\n{call}\n'
                    counts[group] += 1
                path.write_text(content + '  Ok(())\n}\n')
                manifest = json.loads((OUT / 'SEM-3-regex-extract-run.hir.json').read_text())
                manifest['sources'][0]['path'] = path.name
                path.with_suffix('.hir.json').write_text(json.dumps(manifest, indent=2) + '\n')
                files.append(str(path.relative_to(ROOT)))
    passing = ROOT / 'conformance/passing.txt'
    if passing.exists():
        lines = passing.read_text().splitlines()
        passing.write_text('\n'.join(lines + [f for f in files if f not in lines]) + '\n')
    report = '# Regex capture/flag/replacement vectors\n\nGo `' + base.COMMIT + '`. Expectations are copied from the pinned Go/RE2 tables.\n\n'
    report += '\n'.join(f'- {group}: {n} rows' for group, n in counts.items())
    report += f'\n- Corpus programs: {len(files)}\n\nEvery excluded row:\n\n| Source | Reason |\n| --- | --- |\n'
    report += ''.join(f'| {file}:{line} | {reason} |\n' for file, line, reason in excluded)
    (ROOT / 'reports/regex-captures-vectors.md').write_text(report)
    print(dict(counts), len(files), 'programs;', len(excluded), 'excluded rows')

if __name__ == '__main__': main()
