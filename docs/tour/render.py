#!/usr/bin/env python3
"""Refresh the tour's program/output blocks from their conformance sources."""
from pathlib import Path
import re

ROOT = Path(__file__).resolve().parents[2]
PAGE = ROOT / 'docs/tour.md'
PATTERN = re.compile(r'<!-- tour: (conformance/[^\n]+\.hir) -->\n.*?<!-- /tour -->', re.S)


def render(match):
    path = match[1]
    source = (ROOT / path).read_text()
    lines = source.splitlines(keepends=True)
    split = next(i for i, line in enumerate(lines) if not line.startswith('//'))
    body = ''.join(lines[split:]).lstrip('\n')
    output = ''.join(line.removeprefix('// stdout: ').rstrip('\n') + '\n'
                     for line in lines[:split] if line.startswith('// stdout: '))
    exit_code = next((line.removeprefix('// exit: ').strip()
                      for line in lines[:split] if line.startswith('// exit: ')), '0')
    expected = (f'```text\n{output}```' if output else
                'No stdout; exit 0 (all checks pass).' if exit_code == '0' else
                f'No stdout; exit {exit_code}.')
    return (f'<!-- tour: {path} -->\n'
            f'[Runnable source](../{path})\n\n'
            f'```hirsel\n{body}```\n\nExpected:\n\n{expected}\n<!-- /tour -->')


if __name__ == '__main__':
    text = PAGE.read_text()
    title, body = text.split('\n\n', 1)
    link = 'Browse the [full signature index](api.md) for every public core and std declaration.\n\n'
    if not body.startswith(link):
        body = link + body
    PAGE.write_text(PATTERN.sub(render, title + '\n\n' + body))
