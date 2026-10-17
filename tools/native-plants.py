#!/usr/bin/env python3
"""Replay real compiler/runtime mutations against a frozen healthy native cover.

Requires a clean lane worktree. Restores each source byte-for-byte, including
on a failed build or replay. Run after deriving selection.json, before landing.
"""
import argparse
import json
from pathlib import Path
import subprocess
import re
import sys

sys.dont_write_bytecode = True

ROOT = Path(__file__).resolve().parents[1]
BUCK = ROOT / '.buck2/bin/buck2'
COW = 'conformance/OWN/OWN-3-element-sibling-field-block-argument-run.hir'
ADD = 'conformance/SEM/SEM-1-signed-add-fail.hir'
# The production system archive has no leak audit. Overcounting must suppress
# observable resource finalizers, rather than merely leaking a copyable list.
COUNT = 'conformance/DROP/DROP-2-map-replaced-value.hir'
sys.path.insert(0, str(ROOT / 'tools/buck2'))
from gating import PASS, TIMEOUT, test_results  # noqa: E402
from sync import HISTORY_SHARDS  # noqa: E402
PLANTS = [
    ('aarch64-add', ADD, 'crates/hirsel-llvm/src/emit.rs',
     'scalar::binary(&self.builder, op, kind, l, r, &mut guard)',
     'scalar::binary(&self.builder, if self.inputs.target.triple.starts_with("aarch64") && op == BinaryOp::Add { BinaryOp::Sub } else { op }, kind, l, r, &mut guard)'),
    ('system-allocator-count', COUNT, 'crates/hirsel-rt/src/alloc.rs',
     '    pub(crate) unsafe fn allocate(&mut self, index: u32, extent: u64, align: u64) -> *mut Header {\n        // SAFETY: the index is checked; fresh storage is owned here.\n        unsafe {\n            if let Some(capacity) = self.bare_capacity(index, extent, align) {\n                let block = self.allocate_bare(index, capacity);\n                self.set_state(block, STATE_LIVE);\n                return block;\n            }\n            let block = self.allocate_large(extent, align);\n            (*block).count = 1;\n            (*block).descriptor = index | KIND_LARGE << KIND_SHIFT;\n            self.set_state(block, STATE_LIVE);\n            block\n        }\n    }',
     '    pub(crate) unsafe fn allocate(&mut self, index: u32, extent: u64, align: u64) -> *mut Header {\n        // SAFETY: the index is checked; fresh storage is owned here.\n        unsafe {\n            if let Some(capacity) = self.bare_capacity(index, extent, align) {\n                let block = self.allocate_bare(index, capacity);\n                (*block).count = 2;\n                self.set_state(block, STATE_LIVE);\n                return block;\n            }\n            let block = self.allocate_large(extent, align);\n            (*block).count = 2;\n            (*block).descriptor = index | KIND_LARGE << KIND_SHIFT;\n            self.set_state(block, STATE_LIVE);\n            block\n        }\n    }'),
    ('regions-off-release', COW, 'crates/hirsel-driver/src/native/stages.rs',
     '        units.push(seal(StageName::Ready, validate_ready(ready, &inputs))?);',
     '''        let mut ready = ready;
        if !options.regions {
            let mut instructions = Vec::new();
            for block in &mut ready.blocks {
                let kept: Vec<_> = block.instructions.get(&ready.instructions).unwrap().iter()
                    .filter(|inst| !matches!(inst, hirsel_lir::ir::Inst::Release { .. })).copied().collect();
                block.instructions = hirsel_lir::ir::PoolRange::push(&mut instructions, kept);
            }
            ready.instructions = instructions;
        }
        units.push(seal(StageName::Ready, validate_ready(ready, &inputs))?);'''),
]


def pool(targets, config, arguments, output):
    output.mkdir(parents=True, exist_ok=True)
    events = output / 'events.pb.zst'
    # The full gate's own per-test budget, not the pool's shorter default.
    command = [str(BUCK), 'test', *config, '--event-log', str(events), *targets,
               '--', '--timeout', str(TIMEOUT), *['--test-arg=' + arg for arg in arguments]]
    with (output / 'run.log').open('w') as log:
        subprocess.run(command, cwd=ROOT, stdout=log, stderr=subprocess.STDOUT)
    results = test_results(events)
    expected = {target.removeprefix('//:') for target in targets}
    if set(results) != expected:
        raise RuntimeError(f'pool did not run every replay target: {output / "run.log"}')
    for name, result in results.items():
        (output / (name + '.txt')).write_text(result['details'])
    return results


def report(details, key):
    decoder = json.JSONDecoder()
    reports = []
    for match in re.finditer(r'(?m)^\{', details):
        try:
            value, _ = decoder.raw_decode(details[match.start():])
        except ValueError:
            continue
        if isinstance(value, dict) and key in value:
            reports.append(value)
    if len(reports) != 1:
        raise RuntimeError(f'expected one replay report with {key}, got {len(reports)}')
    return reports[0]


def matrix(config, program, output, exhaustive):
    arguments = ['--filter', program, '--seed', '1']
    if exhaustive:
        arguments.append('--exhaustive')
    result = pool(['//:gate-native-full'], config, arguments, output)['gate-native-full']
    value = report(result['details'], 'baseline_minus_native_unlisted')
    if value['baseline_executable'] != 1 or value['stale_exclusions']:
        raise RuntimeError(f'no clean executable control: {output}')
    failures = set()
    for row in value['baseline_minus_native_unlisted']:
        configuration, reason = row.split(' under ', 1)[1].split(': ', 1)
        if configuration == 'snapshot':
            raise RuntimeError(f'no executable native control: {program}')
        failures.add(('heap' if '/heap' in configuration else 'llvm', reason))
    if (not failures and result['status'] != PASS or
            'artifact does not replay or reduce' in result['details']):
        raise RuntimeError(f'pool replay failed independently of findings: {output}')
    return failures, value['configuration_attempts'], result['seconds']


def replay(args, selected, output, config):
    if args.history_only:
        targets = [f'//:gate-native-history-shard-{index}' for index in range(1, HISTORY_SHARDS + 1)]
        results = pool(targets, config, [], output)
        reports = [report(result['details'], 'historical_programs') for result in results.values()]
        total = reports[0]['historical_programs']
        paths = []
        for value in reports:
            if value['historical_programs'] != total or value['lost']:
                raise RuntimeError('inconsistent historical inventory or lost findings')
            paths.extend(row['path'] for row in value['classifications'] + value['unreplayable'])
        if len(paths) != len(set(paths)) or len(paths) != total:
            raise RuntimeError('historical shards did not replay each protected source once')
        if any(result['status'] != PASS for result in results.values()):
            raise RuntimeError('historical pool replay failed')
        value = dict(programs=total, configuration_attempts=sum(r['configuration_attempts'] for r in reports),
                     lost=[], unreplayable=[r for v in reports for r in v['unreplayable']],
                     classifications=[r for v in reports for r in v['classifications']],
                     seeds_per_program=reports[0]['seeds_per_program'],
                     pool_cpus_per_shard=1, slowest_shard_seconds=max(r['seconds'] for r in results.values()))
        (output / 'history.json').write_text(json.dumps(value, indent=2) + '\n')
        print(json.dumps(value, indent=2))
        return
    plants = [plant for plant in PLANTS if not args.plant or plant[0] in args.plant]
    reports = []
    for program in sorted({plant[1] for plant in plants}):
        if program not in selected:
            raise RuntimeError(f'healthy cover omitted planted-bug control: {program}')
        control = output / ('healthy-' + Path(program).stem)
        full, before, before_seconds = matrix(config, program, control / 'exhaustive', True)
        slim, after, after_seconds = matrix(config, program, control / 'slim', False)
        if full or slim:
            raise RuntimeError(f'healthy control already disagrees: {program}')
        reports.append(dict(control=program, healthy=True, exhaustive=before, slim=after,
                            exhaustive_seconds=before_seconds, slim_seconds=after_seconds,
                            pool_cpus=1))
    for name, program, path, original, replacement in plants:
        source = ROOT / path
        saved = source.read_bytes()
        text = saved.decode()
        if text.count(original) != 1:
            raise RuntimeError(f'{name}: mutation anchor must occur exactly once')
        try:
            source.write_text(text.replace(original, replacement))
            full, before, before_seconds = matrix(config, program, output / name / 'exhaustive', True)
            slim, after, after_seconds = matrix(config, program, output / name / 'slim', False)
            if not full or full != slim:
                raise RuntimeError(f'{name}: full={sorted(full)}, slim={sorted(slim)}')
            reports.append(dict(plant=name, detected=True, exhaustive=before, slim=after,
                                exhaustive_seconds=before_seconds, slim_seconds=after_seconds,
                                pool_cpus=1, findings=sorted(full)))
            (output / 'replay.json').write_text(json.dumps(reports, indent=2) + '\n')
        finally:
            source.write_bytes(saved)
    print(json.dumps(reports, indent=2))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--selection', required=True, type=Path)
    parser.add_argument('--output', required=True, type=Path)
    parser.add_argument('--history-only', action='store_true')
    parser.add_argument('--plant', action='append', choices=[p[0] for p in PLANTS])
    args = parser.parse_args()
    if subprocess.check_output(['git', 'status', '--porcelain'], cwd=ROOT, text=True).strip():
        parser.error('commit the lane changes before running transient mutations')
    selection = args.selection.resolve()
    selected = set(json.loads(selection.read_text())['selected'])
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    frozen = ROOT / '.native-diet-selection.json'
    with frozen.open('xb') as target:
        target.write(selection.read_bytes())
    config = ['-c', 'hirsel.full=true', '-c', 'hirsel.tests=focus',
              '-c', 'hirsel.native_selection_file=' + frozen.name]
    try:
        replay(args, selected, output, config)
    finally:
        frozen.unlink()


if __name__ == '__main__':
    main()
