#!/usr/bin/env python3
"""Ten-round interleaved local primitives. No multicore-executor speed claim."""
import argparse
import fcntl
import hashlib
import json
from pathlib import Path
import random
import statistics
import subprocess
import tempfile

HERE = Path(__file__).resolve().parent
MODES = ('bare', 'stable', 'refresh', 'handoff')


def cpu_stat():
    result = {}
    for line in Path('/proc/stat').read_text().splitlines():
        words = line.split()
        if words and words[0].startswith('cpu') and words[0][3:].isdigit():
            values = list(map(int, words[1:9]))
            result[int(words[0][3:])] = (sum(values), values[3] + values[4])
    return result


def measure(output, iterations):
    rng = random.Random(20261020)
    samples = []
    with tempfile.TemporaryDirectory(prefix='mc-profile-2-') as temp:
        binary = Path(temp) / 'cost'
        subprocess.run(['cc', '-O3', '-Wall', '-Wextra', '-Werror', '-pthread',
                        str(HERE/'cost.c'), str(HERE/'switch.S'), '-o', str(binary)], check=True)
        binary_sha256 = hashlib.sha256(binary.read_bytes()).hexdigest()
        for round_ in range(10):
            cells = [(w, m) for w in (4, 8) for m in range(4)]
            rng.shuffle(cells)
            for width, mode in cells:
                count = iterations if mode != 3 else max(10000, iterations // 100)
                counters = Path(temp) / 'perf'
                before = cpu_stat()
                run = subprocess.run(['perf', 'stat', '-x;', '-o', str(counters),
                                      '-e', 'cycles,instructions', '--', str(binary),
                                      str(width), str(mode), str(count)],
                                     text=True, capture_output=True, check=True)
                after = cpu_stat()
                sample = json.loads(run.stdout)
                assert sample['checksum'] == count * width
                sample.update(round=round_, variant=MODES[mode])
                sample['perf_raw'] = counters.read_text()
                for line in sample['perf_raw'].splitlines():
                    fields = line.split(';')
                    if len(fields) > 4 and fields[2] in ('cycles', 'instructions'):
                        sample[fields[2]] = int(fields[0])
                        assert float(fields[4]) >= 95, 'multiplexed perf sample'
                assert all(sample.get(k, 0) > 0 for k in ('cycles', 'instructions'))
                sample['smt_busy'] = {}
                for cpu in range(32-width, 32):
                    sibling = cpu - 16
                    total = after[sibling][0] - before[sibling][0]
                    idle = after[sibling][1] - before[sibling][1]
                    sample['smt_busy'][str(sibling)] = 1-idle/total if total else None
                samples.append(sample)
                print(round_, width, MODES[mode], round(sample['ns_per_unit'], 2), flush=True)
    summary = []
    for width in (4, 8):
        rows = [s for s in samples if s['width'] == width]
        by = {(s['round'], s['variant']): s for s in rows}
        for variant in MODES:
            selected = [s for s in rows if s['variant'] == variant]
            ratios = [s['ns_per_unit']/by[s['round'], 'stable']['ns_per_unit'] for s in selected]
            insns = [s['instructions']/s['checksum'] for s in selected]
            instruction_ratios = [(s['instructions']/s['checksum']) /
                                  (by[s['round'], 'stable']['instructions']/
                                   by[s['round'], 'stable']['checksum']) for s in selected]
            summary.append(dict(width=width, variant=variant,
                                ns=statistics.median(s['ns_per_unit'] for s in selected),
                                ns_ratio_to_stable=statistics.median(ratios),
                                ns_ratio_range=[min(ratios), max(ratios)],
                                instructions=statistics.median(insns),
                                instruction_ratio_to_stable=statistics.median(instruction_ratios)))
    output.write_text(json.dumps(dict(schema=1, seed=20261020, rounds=10,
        machine=subprocess.check_output(['uname', '-a'], text=True).strip(),
        binary_sha256=binary_sha256,
        revision=subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip(),
        sources={p.name: hashlib.sha256(p.read_bytes()).hexdigest()
                 for p in (HERE/'cost.c', HERE/'switch.S', HERE/'measure.py')},
        compiler=subprocess.check_output(['cc', '--version'], text=True).splitlines()[0],
        summary=summary, samples=samples), indent=2)+'\n')


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, default=HERE/'costs.json')
    parser.add_argument('--iterations', type=int, default=2000000)
    args = parser.parse_args()
    lock = Path('/workspace/code/hirsel-briefs/locks/cpu31-timing.lock')
    with lock.open('a') as stream:
        print('Waiting for CPU31 timing lease', flush=True)
        fcntl.flock(stream, fcntl.LOCK_EX)
        measure(args.output, args.iterations)
