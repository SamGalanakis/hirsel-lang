#!/usr/bin/env python3
"""Land throughput, from the logs tools/land appends in the clone's common
git directory.

    tools/land_stats.py [--since 2026-10-19T15:00:00Z] [--until ...]

Lands per hour counts landed changes (`hirsel-landings.jsonl`) over the span
from the first to the last in the window, beside the median land wait and,
for batched landings, the batch sizes. Batches (`hirsel-land-batches.jsonl`)
give the median batch time, green and red apart, and what red batches spent
on checks after their first: the bisection and the rebuild of the rest, and
every flake (a failure that passed when repeated) with its batch.
"""

import argparse
import json
from pathlib import Path
import statistics
import subprocess
import time


def rows(path, since, until):
    try:
        lines = path.read_text().splitlines()
    except FileNotFoundError:
        return []
    found = [json.loads(line) for line in lines if line.strip()]
    return [row for row in found if since <= row.get('at', '') <= until]


def stamp(text):
    return time.mktime(time.strptime(text, '%Y-%m-%dT%H:%M:%SZ'))


def median(values):
    values = [value for value in values if value is not None]
    return f'{statistics.median(values) / 60:.1f} min' if values else '-'


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('--since', default='')
    parser.add_argument('--until', default='9')
    args = parser.parse_args()
    common = Path(subprocess.run(['git', 'rev-parse', '--path-format=absolute', '--git-common-dir'],
                                 capture_output=True, text=True, check=True).stdout.strip())
    lands = rows(common / 'hirsel-landings.jsonl', args.since, args.until)
    if lands:
        hours = max(stamp(lands[-1]['at']) - stamp(lands[0]['at']), 1) / 3600
        batched = [row['batch'] for row in lands if row.get('batch')]
        print(f'lands: {len(lands)} from {lands[0]["at"]} to {lands[-1]["at"]}: {len(lands) / hours:.1f} per hour; '
              f'median wait {median(r.get("land_wait_seconds") for r in lands)}, '
              f'median held {median(r.get("land_held_seconds") for r in lands if r.get("land_held_seconds"))}'
              + (f'; {len(batched)} in batches of {statistics.mean(len(b) for b in batched):.1f} on average'
                 if batched else ''))
    else:
        print('lands: none in the window')
    batches = rows(common / 'hirsel-land-batches.jsonl', args.since, args.until)
    if batches:
        red = [row for row in batches if not all(check['passed'] for check in row['checks'])]
        green = [row for row in batches if row not in red]
        print(f'batches: {len(batches)}, median {median(r["seconds"] for r in batches)} '
              f'(green {median(r["seconds"] for r in green)}, red {median(r["seconds"] for r in red)}); '
              f'{sum(len(r["checks"]) for r in batches) / len(batches):.1f} checks per batch; '
              f'{len(red)} red spent {sum(c["seconds"] for r in red for c in r["checks"][1:]) / 60:.1f} min '
              'on checks after their first')
        for row in batches:
            for flake in row.get('flakes', []):
                print(f'flake: {", ".join(flake["failed"])} on {flake["tip"][:12]} at {row["at"]} '
                      f'(batch: {", ".join(flake["batch"])})')
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
