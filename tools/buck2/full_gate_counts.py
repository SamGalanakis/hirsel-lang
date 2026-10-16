#!/usr/bin/env python3
"""Check full-gate coverage from Buck2's combined test output, per profile."""
import argparse
from collections import Counter
from pathlib import Path
import re
import subprocess
import sys

ANSI = re.compile(r'\x1b\[[0-9;]*m')
TIMESTAMP = re.compile(r'^\[[^\]]+\]\s*')
RESULT = re.compile(r'^test (\S+)(?: - should panic)? \.\.\. (ok|FAILED|ignored)\b')


class Counts:
    def __init__(self):
        self.expected = set()
        self.actual = Counter()
        self.binary = None
        self.failures = []

    def observe(self, line):
        line = TIMESTAMP.sub('', ANSI.sub('', line)).strip()
        if line == '---- STDOUT ----':
            self.binary = None
        elif line.startswith('HIRSEL_FULL_GATE_BINARY '):
            self.binary = line.split()[1]
        elif line.startswith('HIRSEL_FULL_GATE_EXPECT '):
            _, binary, case = line.split()
            self.expected.add((binary, case))
        elif match := RESULT.match(line):
            case, status = match.groups()
            if self.binary is None:
                self.failures.append(f'{case}: missing binary inventory marker')
            elif status != 'ignored':
                self.actual[self.binary, case] += 1
                if status != 'ok':
                    self.failures.append(f'{self.binary} {case}: {status}')

    def finish(self):
        problems = list(self.failures)
        if not self.expected:
            problems.append('no compiled full-gate inventory found')
        for binary, case in sorted(self.expected | self.actual.keys()):
            count = self.actual[binary, case]
            if (binary, case) not in self.expected:
                problems.append(f'{binary} {case}: unexpected execution ({count})')
            elif count != 1:
                problems.append(f'{binary} {case}: expected once, observed {count}')
        if problems:
            raise ValueError('; '.join(problems))
        return len(self.expected)


def main():
    parser = argparse.ArgumentParser(description=__doc__,
                                     usage='%(prog)s LOG | --run COMMAND ...')
    parser.add_argument('log', type=Path)
    # Forward the child argv verbatim: argparse treats Buck2's `--` separator
    # specially even with REMAINDER on an optional argument.
    command = sys.argv[2:] if sys.argv[1:2] == ['--run'] else None
    if command == []:
        parser.error('--run requires a command')
    log_path = None if command is not None else parser.parse_args().log
    counts = Counts()
    if command is not None:
        with subprocess.Popen(command, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                              text=True, errors='replace') as child:
            for line in child.stdout:
                print(line, end='', flush=True)
                counts.observe(line)
            status = child.wait()
        if status:
            return status
    else:
        with log_path.open(errors='replace') as log:
            for line in log:
                counts.observe(line)
    try:
        total = counts.finish()
    except ValueError as error:
        print(f'full gate coverage: {error}', file=sys.stderr)
        return 1
    print(f'full gate coverage: {total} compiled cases each executed exactly once across pool and host')
    return 0


if __name__ == '__main__':
    sys.exit(main())
