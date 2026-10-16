#!/usr/bin/env python3
"""Summarize one scheduled full-gate log as latest.json (tools/main-full-run.sh).

Usage: main_full_result.py RUNS SHA START END STATUS FALLBACK_STAGE LOG DURATION

The log mixes bare gate lines with Buck2 output, which prefixes each line with
`[<timestamp>] ` and colours its summaries. Every line is read without them.
"""
import json
import os
import re
import sys
from pathlib import Path

TIMESTAMP = re.compile(r'^\[\d{4}-\d\d-\d\dT[^\]]*\] ?', re.M)
COLOUR = re.compile(r'\x1b\[[0-9;]*m')
# `check.sh: <stage> passed (12 s)` and `check.sh: <stage> failed (exit 1, 12 s)`.
STAGE = re.compile(r'^(check(?:-native)?\.sh): (.+) (?:passed \((\d+) s\)|failed \(exit (\d+), (\d+) s\))$', re.M)
# Buck2 names a failed test target in its result line and again in the summary.
TARGET = re.compile(r'^(?:✗ (?:Fail|Timeout|Fatal|Infra Failure): |  ✗ )(\S+//:\S+)', re.M)
# A target the pool never started: empty output and a remote execution error.
INFRA = re.compile(r'^✗ [A-Za-z ]+: (\S+//:\S+).*\n---- STDOUT ----\n\n---- STDERR ----\n'
                   r'(?:Remote Execution Error|Error: \(Failed to start remote execution)', re.M)


def plain(text):
    """The log as the gate printed it: no Buck2 timestamps or colours."""
    return TIMESTAMP.sub('', COLOUR.sub('', text))


def stages(text):
    """Every stage of check.sh and check-native.sh in order, with its result."""
    out = []
    for match in STAGE.finditer(text):
        gate, name, passed, status, failed = match.groups()
        out.append(dict(gate=gate, stage=name, result='pass' if passed else 'fail',
                        exit=0 if passed else int(status), seconds=int(passed or failed)))
    return out


def failing_targets(text):
    """Failed Buck2 test targets, and those among them the pool never ran."""
    infra = set(INFRA.findall(text))
    return {t for t in TARGET.findall(text) if t not in infra}, infra


def failing_tests(text, failed_stage):
    tests = set(re.findall(r'^test (.+?) \.\.\. FAILED$', text, re.M))
    tests.update(re.findall(r'^---- (.+?) stdout ----$', text, re.M))
    tests.update(re.findall(r'^(conformance/\S+\.hir):', text, re.M))
    tests.update(re.findall(r'^(?:smoke|real Io replay|memcheck|topology|generated|hash sweep): (.+?)(?: under [^:\n]+)?:', text, re.M))
    tests.update(re.findall(r'^native: (\S+)\s+(?:heap|llvm)(?:\s|:)', text, re.M))
    # Agreement reports identify the corpus programs behind discrepancy clusters.
    # Known native kinds can occur in a passing run: only new kinds are failures.
    if failed_stage and 'agreement' in failed_stage:
        reports = []
        for match in re.finditer(r'^\{', text, re.M):
            try:
                report, _ = json.JSONDecoder().raw_decode(text[match.start():])
                if isinstance(report, dict) and 'clusters' in report:
                    reports.append(report)
            except json.JSONDecodeError:
                pass
        if reports:
            kinds = set(re.findall(r'^agree: new discrepancy kind: (.+)$', text, re.M))
            tests.update(c['path'] for c in reports[-1]['clusters']
                         if 'path' in c and ('known_kinds' not in reports[-1] or c.get('kind') in kinds))
    return tests


def summarize(text, status, fallback):
    text = plain(text)
    run = stages(text)
    failed = next((s['stage'] for s in run if s['result'] == 'fail'), None)
    targets, infra = failing_targets(text)
    return dict(result='pass' if status == 0 else 'fail',
                first_failing_stage=None if status == 0 else failed or fallback,
                failing_tests=sorted(failing_tests(text, failed) | targets),
                infra_failures=sorted(infra), stages=run)


def main(runs, sha, start, end, status, fallback, logfile, duration):
    status = int(status)
    text = Path(logfile).read_text(errors='replace') if logfile else ''
    result = dict(sha=sha, start=start, end=end) | summarize(text, status, fallback)
    result |= dict(duration_seconds=int(duration), log=logfile)
    # One complete JSON line by atomic rename.
    latest = Path(runs) / 'latest.json'
    temporary = latest.with_name(f'.latest-{os.getpid()}.json')
    temporary.write_text(json.dumps(result, separators=(',', ':')) + '\n')
    temporary.replace(latest)


if __name__ == '__main__':
    if len(sys.argv) != 9:
        sys.exit(__doc__.strip().splitlines()[2])
    main(*sys.argv[1:])
