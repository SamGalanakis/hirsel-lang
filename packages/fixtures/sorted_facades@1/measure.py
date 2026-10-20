#!/usr/bin/env python3
"""Recompute the fixed survey ledger's source savings; no build or execution."""
import json
from pathlib import Path
import re
import subprocess

ROOT = Path(__file__).resolve().parents[3]
PILOT = Path(__file__).resolve().parent
# These fixtures contain ordinary strings, integer literals and ASCII identifiers.
# Longest punctuation first, as hirsel-lexer; comments/whitespace are trivia.
TOKEN = re.compile(r'//[^\n]*|"(?:\\.|[^"\\])*"|[A-Za-z_][A-Za-z_0-9]*|'
                   r'[0-9][A-Za-z_0-9]*|\.\.=|<<=|>>=|\.\.|->|=>|&&|\|\||'
                   r'<<|>>|==|!=|<=|>=|\+=|-=|\*=|/=|%=|&=|\|=|\^=|[^\s]')


def tokens(text):
    return [t for t in TOKEN.findall(text) if not t.startswith('//')]


def main():
    ledger = json.loads((PILOT / 'ledger.json').read_text())
    totals = dict(survey_sites=0, migrated=0, blocked_empty=0,
                  blocked_resource_remove=0, before_tokens=0, after_tokens=0,
                  before_lines=0, after_lines=0)
    operations = dict.fromkeys(('empty', 'insert', 'len', 'get', 'remove'), 0)
    rows = []
    for site in ledger['sites']:
        path = site['path']
        before = subprocess.check_output(
            ['git', 'show', f'{ledger["baseline"]}:{path}'], cwd=ROOT, text=True)
        after = (ROOT / path).read_text()
        calls = {op: len(re.findall(r'_maps\.' + op + r'\(', after)) for op in operations}
        assert calls == site['migrated'], (path, calls)
        count = sum(calls.values())
        assert count + site['blocked_empty'] + site['blocked_resource_remove'] == site['survey_sites']
        # Apart from the named module and removed witness operands, the token
        # sequence must be identical. Includes loans, take/mut and annotations.
        restored = after
        for op in operations:
            restored = re.sub(r'u(?:32|64)_maps\.' + op + r'\(',
                              'sorted_map.' + op + '(by, ', restored)
        assert tokens(restored) == tokens(before), path
        bt, at = len(tokens(before)), len(tokens(after))
        assert bt - at == 2 * count, (path, bt, at)
        row = dict(path=path, migrated=count, tokens_removed=bt-at,
                   before_lines=len(before.splitlines()), after_lines=len(after.splitlines()))
        assert row['before_lines'] == site['before_lines']
        assert row['after_lines'] == site['after_lines']
        rows.append(row)
        for key in ('survey_sites', 'blocked_empty', 'blocked_resource_remove'):
            totals[key] += site[key]
        totals['migrated'] += count
        totals['before_tokens'] += bt
        totals['after_tokens'] += at
        totals['before_lines'] += row['before_lines']
        totals['after_lines'] += row['after_lines']
        for op, n in calls.items():
            operations[op] += n
    assert totals['survey_sites'] == 87
    assert totals['migrated'] == 65
    assert operations == dict(empty=0, insert=32, len=13, get=11, remove=9)
    totals['tokens_removed'] = totals['before_tokens'] - totals['after_tokens']
    totals['lines_removed'] = totals['before_lines'] - totals['after_lines']
    wrappers = {p.name: len(p.read_text().splitlines()) for p in PILOT.glob('*_maps.hir')}
    print(json.dumps(dict(totals=totals, operations=operations, facade_lines=wrappers,
                         sites=rows), indent=2))


if __name__ == '__main__':
    main()
