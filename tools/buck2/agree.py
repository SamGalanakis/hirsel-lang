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

import event_logs

from conformance import BUCK2, CONFIG, ROOT, shard_outputs
from sync import (AGREE_FULL_SHARDS, AGREE_FULL_TIMEOUT, AGREE_LIMIT, AGREE_NATIVE_FULL_SHARDS,
                  AGREE_NATIVE_FULL_TIMEOUT, AGREE_SHARDS)

COUNTS = ('checked', 'skipped', 'rejected', 'valid', 'unavailable', 'runtime_limits', 'executed')
GLOBAL = ('seed', 'limit', 'hash_seed', 'programs', 'generated', 'eligible', 'variants_per_program')
# The limit an unrestricted report carries (agree's default, usize::MAX).
UNLIMITED = 2**64 - 1
KNOWN = ROOT / 'native-tests/regressions/agree-known.txt'
# A cluster's first variant, streamed to stderr as a shard finds it (agree.rs).
STREAMED = re.compile(r'(?m)^(?:\[[^\]]*\] )?agree: cluster (\{.*\})$')


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
    if (limit != UNLIMITED and total['seed'] != 1) or total['limit'] != limit or total['hash_seed'] != 0:
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
    if total['checked'] + total['skipped'] != min(limit, total['eligible']):
        raise ValueError('agreement shards did not cover the global sample')
    if total['clusters']:
        raise ValueError('agreement shards flagged discrepancies: ' + json.dumps(total['clusters']))
    if total['checked'] != total['rejected'] + total['valid'] + total['unavailable']:
        raise ValueError('agreement classifications do not sum to checked')
    if sum(total['edits'].values()) != total['checked']:
        raise ValueError('agreement edit counts do not sum to checked')
    return total


def known_kinds(text):
    return {line for line in text.splitlines() if line and not line.startswith('#')}


def merge_native(outputs, shards, listed):
    """The union of the native shards' clusters, judged against the listed kinds.

    A shard that printed its report contributes its reduced clusters; one that
    stopped first (a timeout) contributes the clusters it streamed, and the
    merge then cannot tell which listed kinds no longer occur. Returns the
    merged report and the errors that fail the stage.
    """
    errors = []
    clusters = {}
    complete = set()
    sample = None
    totals = {key: 0 for key in ('checked', 'skipped', 'valid', 'executed', 'native_agreed')}
    for name, output in sorted(outputs.items()):
        try:
            report = report_from_output(output)
        except ValueError:
            report = None
        if report is not None:
            shard = report.get('shard', {})
            index = shard.get('index')
            if shard.get('count') != shards or index not in range(1, shards + 1) or index in complete:
                errors.append(f'{name}: duplicate or invalid native agreement shard: {shard}')
                continue
            # The seed rotates per full run, so the shards must agree on it,
            # on the cover they sampled and on its budget, not on one value.
            parameters = {key: report.get(key) for key in GLOBAL}
            if report.get('limit') != UNLIMITED or report.get('hash_seed') != 0 or (
                    sample is not None and parameters != sample):
                errors.append(f'{name}: agreement parameters differ from the gate')
                continue
            sample = parameters
            complete.add(index)
            for key in totals:
                totals[key] += report.get(key, 0)
            found = report['clusters']
        else:
            errors.append(f'{name} stopped before its report; its streamed clusters are kept')
            found = [json.loads(match[1]) for match in STREAMED.finditer(output)]
        for cluster in found:
            key = (cluster['kind'], cluster['edit'])
            if key not in clusters or report is not None:
                clusters[key] = cluster
    if len(complete) != shards:
        missing = sorted(set(range(1, shards + 1)) - complete)
        errors.append(f'{shards - len(missing)} of {shards} native agreement shards reported; '
                      f'missing {missing}')
    elif totals['checked'] + totals['skipped'] != min(sample['limit'], sample['eligible']):
        errors.append(f"native agreement shards checked and skipped {totals['checked'] + totals['skipped']} "
                      f"of {sample['eligible']} eligible variants")
    kinds = {kind for kind, _ in clusters}
    for kind in sorted(kinds - listed):
        errors.append(f'new discrepancy kind: {kind}')
    if len(complete) == shards:
        for kind in sorted(listed - kinds):
            errors.append(f'{kind} no longer occurs; delete it from {KNOWN.relative_to(ROOT)}')
    merged = dict(totals, seed=sample and sample['seed'], eligible=sample and sample['eligible'],
                  shards=shards, complete=len(complete), known_kinds=len(listed),
                  clusters=[clusters[key] for key in sorted(clusters)])
    return merged, errors


def native_full():
    """The native shards in one Buck2 run; a shard that failed or stopped still
    contributes what its output holds."""
    targets = [f'//:gate-agree-native-full-shard-{index}' for index in range(1, AGREE_NATIVE_FULL_SHARDS + 1)]
    with event_logs.recorded('agree') as event_log:
        status = subprocess.call([str(BUCK2), 'test', *CONFIG, '--event-log', str(event_log), *targets,
                                  '--', '--timeout', str(AGREE_NATIVE_FULL_TIMEOUT)], cwd=ROOT)
        outputs = shard_outputs(event_log) if event_log.exists() else {}
    merged, errors = merge_native(outputs, AGREE_NATIVE_FULL_SHARDS, known_kinds(KNOWN.read_text()))
    print(json.dumps(merged, indent=2, sort_keys=True))
    for error in errors:
        print(f'agree: {error}', file=sys.stderr)
    return 1 if errors or status else 0


def main():
    if sys.argv[1:] == ['--native-full']:
        return native_full()
    if sys.argv[1:] == ['--full']:
        name, shards, limit, timeout = 'gate-agree-full-shard', AGREE_FULL_SHARDS, UNLIMITED, AGREE_FULL_TIMEOUT
    elif sys.argv[1:]:
        print(__doc__, file=sys.stderr)
        return 2
    else:
        name, shards, limit, timeout = 'gate-agree-shard', AGREE_SHARDS, AGREE_LIMIT, 300
    targets = [f'//:{name}-{index}' for index in range(1, shards + 1)]
    with event_logs.recorded('agree') as event_log:
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
