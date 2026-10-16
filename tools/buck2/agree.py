#!/usr/bin/env python3
"""Run the agreement shards; refuse missing/duplicate shards and sum reports.

Without arguments: the push gate's AGREE_LIMIT ranked variants over AGREE_SHARDS
shards. With --full: the unrestricted seed-1 campaign over AGREE_FULL_SHARDS
shards, each under its own AGREE_FULL_TIMEOUT (`tools/gate --full`).
"""

import json
from pathlib import Path
import re
import subprocess
import sys
import tempfile

from conformance import BUCK2, CONFIG, ROOT, shard_outputs
from sync import AGREE_FULL_SHARDS, AGREE_FULL_TIMEOUT, AGREE_LIMIT, AGREE_SHARDS

COUNTS = ('checked', 'skipped', 'rejected', 'valid', 'unavailable', 'runtime_limits', 'executed')
GLOBAL = ('seed', 'limit', 'hash_seed', 'programs', 'generated')
# The limit an unrestricted report carries (agree's default, usize::MAX).
UNLIMITED = 2**64 - 1


def report_from_output(output):
    decoder = json.JSONDecoder()
    reports = []
    for start in re.finditer(r'(?m)^\{', output):
        try:
            value, _ = decoder.raw_decode(output[start.start():])
        except ValueError:
            continue
        if isinstance(value, dict) and 'checked' in value:
            reports.append(value)
    if len(reports) != 1:
        raise ValueError(f'expected one agreement report, got {len(reports)}')
    return reports[0]


def summarize(outputs, shards=AGREE_SHARDS, limit=AGREE_LIMIT):
    if len(outputs) != shards:
        raise ValueError(f'{len(outputs)} of {shards} agreement shards reported')
    reports = [report_from_output(output) for output in outputs.values()]
    first = reports[0]
    total = {key: first[key] for key in GLOBAL}
    if total['seed'] != 1 or total['limit'] != limit or total['hash_seed'] != 0:
        raise ValueError('agreement sample parameters differ from the gate')
    total.update({key: 0 for key in COUNTS})
    total.update(edits={}, clusters=[])
    seen = set()
    for report in reports:
        shard = report.get('shard', {})
        index = shard.get('index')
        if shard.get('count') != shards or index not in range(1, shards + 1) or index in seen:
            raise ValueError(f'duplicate or invalid agreement shard: {shard}')
        seen.add(index)
        if any(report[key] != total[key] for key in GLOBAL):
            raise ValueError(f'shard {index}: global catalogue/sample counts differ')
        for key in COUNTS:
            value = report[key]
            if type(value) is not int or value < 0:
                raise ValueError(f'shard {index}: invalid {key}')
            total[key] += value
        for edit, count in report['edits'].items():
            total['edits'][edit] = total['edits'].get(edit, 0) + count
        total['clusters'].extend(report['clusters'])
    if total['checked'] + total['skipped'] != min(limit, total['generated']):
        raise ValueError('agreement shards did not cover the global sample')
    if total['clusters']:
        raise ValueError('agreement shards flagged discrepancies: ' + json.dumps(total['clusters']))
    if total['checked'] != total['rejected'] + total['valid'] + total['unavailable']:
        raise ValueError('agreement classifications do not sum to checked')
    if sum(total['edits'].values()) != total['checked']:
        raise ValueError('agreement edit counts do not sum to checked')
    return total


def main():
    if sys.argv[1:] == ['--full']:
        name, shards, limit, timeout = 'gate-agree-full-shard', AGREE_FULL_SHARDS, UNLIMITED, AGREE_FULL_TIMEOUT
    elif sys.argv[1:]:
        print(__doc__, file=sys.stderr)
        return 2
    else:
        name, shards, limit, timeout = 'gate-agree-shard', AGREE_SHARDS, AGREE_LIMIT, 300
    targets = [f'//:{name}-{index}' for index in range(1, shards + 1)]
    with tempfile.TemporaryDirectory() as scratch:
        event_log = Path(scratch) / 'events.pb.zst'
        status = subprocess.call([str(BUCK2), 'test', *CONFIG, '--event-log', str(event_log), *targets,
                                  '--', '--timeout', str(timeout)], cwd=ROOT)
        if status:
            return status
        outputs = shard_outputs(event_log)
    try:
        total = summarize(outputs, shards, limit)
    except (ValueError, KeyError, TypeError) as error:
        print(f'agree.py: {error}', file=sys.stderr)
        return 1
    print(f'agree.py: {shards} shards')
    print(json.dumps(total, indent=2, sort_keys=True))
    return 0


if __name__ == '__main__':
    sys.exit(main())
