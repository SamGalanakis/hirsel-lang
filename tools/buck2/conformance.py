#!/usr/bin/env python3
"""Run the conformance validation and ratchet as its Buck2 shards and sum their counts.

Each `//:gate-conformance-shard-<i>` runs `hirsel-conformance --check-format
--shard <i>/<n>` over the corpus files whose path hashes to it (tools/buck2/sync.py,
CONFORMANCE_SHARDS). Buck2 prints a test's output only when it fails, so this
reads the shards' output from the run's event log and prints the whole run's
counts in `cargo run -p hirsel-conformance -- --check-format`'s lines. It fails
when a shard fails or a shard's counts are missing.

    python3 tools/buck2/conformance.py            # the per-push stage
    python3 tools/buck2/conformance.py --self-test
"""

import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[2]
BUCK2 = ROOT / '.buck2/bin/buck2'
# The configuration `tools/gate --full` runs Buck2 with, so the daemon keeps one state.
CONFIG = os.environ.get('HIRSEL_BUCK2_CONFIG', '').split()
sys.path.insert(0, str(Path(__file__).resolve().parent))
from sync import CONFORMANCE_SHARDS  # noqa: E402

COUNTS = re.compile(r'^(whole corpus|in-gate corpus): (\d+) validated / (\d+) passing / (\d+) failing / (\d+) not implemented$')
EXEMPT = re.compile(r'^corpus formatting: (\d+) intentional syntax rejections exempt$')


def summarize(outputs, shards):
    """The summed count lines of the shards' outputs, or an error naming what they lack."""
    if len(outputs) != shards:
        raise ValueError(f'{len(outputs)} of {shards} conformance shards reported')
    totals = {'whole corpus': [0, 0, 0, 0], 'in-gate corpus': [0, 0, 0, 0]}
    exempt = 0
    for shard, output in sorted(outputs.items()):
        seen = set()
        for line in output.splitlines():
            if match := COUNTS.match(line.strip()):
                seen.add(match[1])
                totals[match[1]] = [a + int(b) for a, b in zip(totals[match[1]], match.groups()[1:])]
            elif match := EXEMPT.match(line.strip()):
                seen.add('formatting')
                exempt += int(match[1])
        if missing := {'whole corpus', 'in-gate corpus', 'formatting'} - seen:
            raise ValueError(f'{shard} reported no counts for: ' + ', '.join(sorted(missing)))
    lines = [f'{name}: {v} validated / {p} passing / {f} failing / {n} not implemented'
             for name, (v, p, f, n) in totals.items()]
    return lines + [f'corpus formatting: {exempt} intentional syntax rejections exempt']


def shard_outputs(event_log):
    """Each shard's captured output, from the TestResult events of a `buck2 test` event log."""
    events = subprocess.run([str(BUCK2), 'log', 'show', str(event_log)], cwd=ROOT, check=True,
                            stdout=subprocess.PIPE, text=True, errors='replace').stdout
    outputs = {}
    for line in events.splitlines():
        if '"TestResult"' not in line:
            continue
        data = json.loads(line).get('Event', {}).get('data', {}).get('Instant', {}).get('data', {})
        if result := data.get('TestResult'):
            outputs[result['name']] = result.get('details') or ''
    return outputs


def self_test():
    shard = ('---- STDOUT ----\n'
             'whole corpus: 3 validated / 2 passing / 0 failing / 1 not implemented\n'
             'in-gate corpus: 2 validated / 2 passing / 0 failing / 0 not implemented\n'
             'corpus formatting: 1 intentional syntax rejections exempt\n')
    assert summarize({'a': shard, 'b': shard}, 2) == [
        'whole corpus: 6 validated / 4 passing / 0 failing / 2 not implemented',
        'in-gate corpus: 4 validated / 4 passing / 0 failing / 0 not implemented',
        'corpus formatting: 2 intentional syntax rejections exempt',
    ]
    for outputs, error in [({'a': shard}, '1 of 2'), ({'a': shard, 'b': shard.split('in-gate')[0]}, 'b reported')]:
        try:
            summarize(outputs, 2)
        except ValueError as raised:
            assert error in str(raised), raised
        else:
            raise AssertionError(f'{error} must fail')
    print('conformance.py: self-test passed')
    return 0


def main():
    if sys.argv[1:] == ['--self-test']:
        return self_test()
    if sys.argv[1:]:
        print(__doc__, file=sys.stderr)
        return 2
    targets = [f'//:gate-conformance-shard-{index}' for index in range(1, CONFORMANCE_SHARDS + 1)]
    with tempfile.TemporaryDirectory() as scratch:
        event_log = Path(scratch) / 'events.pb.zst'
        status = subprocess.call([str(BUCK2), 'test', *CONFIG, '--event-log', str(event_log), *targets,
                                  '--', '--timeout', '1800'], cwd=ROOT)
        if status != 0:
            return status
        outputs = shard_outputs(event_log)
    try:
        lines = summarize(outputs, CONFORMANCE_SHARDS)
    except ValueError as error:
        print(f'conformance.py: {error}', file=sys.stderr)
        return 1
    print(f'conformance.py: {CONFORMANCE_SHARDS} shards')
    print('\n'.join(lines))
    return 0


if __name__ == '__main__':
    sys.exit(main())
