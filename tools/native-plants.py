#!/usr/bin/env python3
"""Replay real compiler/runtime mutations against a frozen healthy native cover.

Requires a clean lane worktree. Restores each source byte-for-byte, including
on a failed build or replay. Run after deriving selection.json, before landing.
"""
import argparse
import json
import os
from pathlib import Path
import subprocess

ROOT = Path(__file__).resolve().parents[1]
BUCK = ROOT / '.buck2/bin/buck2'
LOCK = '/workspace/code/hirsel-briefs/locks/native-campaign.lock'
COW = 'conformance/OWN/OWN-3-element-sibling-field-block-argument-run.hir'
ADD = 'conformance/SEM/SEM-1-signed-add-fail.hir'
PLANTS = [
    ('aarch64-add', ADD, 'crates/hirsel-llvm/src/emit.rs',
     'scalar::binary(&self.builder, op, kind, l, r, &mut guard)',
     'scalar::binary(&self.builder, if self.inputs.target.triple.starts_with("aarch64") && op == BinaryOp::Add { BinaryOp::Sub } else { op }, kind, l, r, &mut guard)'),
    ('system-allocator-count', COW, 'crates/hirsel-rt/src/count.rs',
     '(*block).count = count + 1;',
     '(*block).count = count + u32::from(!cfg!(feature = "system-allocator"));'),
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


def build():
    command = [str(BUCK), 'build', '--target-platforms', '//tools/buck2:release',
               '//:hirsel-conformance-bin-native', '//:hirsel-rt-archives', '--show-full-json-output']
    result = subprocess.run(command, cwd=ROOT, check=True, text=True, stdout=subprocess.PIPE)
    outputs = {target.rsplit(':', 1)[-1]: path for target, path in json.loads(result.stdout).items()}
    return outputs['hirsel-conformance-bin-native'], outputs['hirsel-rt-archives']


def matrix(binary, archives, selection, program, output, exhaustive):
    env = os.environ | {'HIRSEL_RUNTIME_ARCHIVES': str(Path(archives).resolve()),
                       'HIRSEL_NATIVE_SELECTION': str(selection),
                       'LLVM_SYS_211_PREFIX': str(ROOT / 'tools/buck2/toolchains/llvm-files')}
    command = ['flock', LOCK, binary, '--full', '--root', str(ROOT), '--seed', '1',
               '--filter', program, '--out', str(output)]
    if exhaustive:
        command.append('--exhaustive')
    result = subprocess.run(command, cwd=ROOT, env=env, text=True, stdout=subprocess.PIPE,
                            stderr=subprocess.STDOUT)
    output.mkdir(parents=True, exist_ok=True)
    (output / 'run.log').write_text(result.stdout)
    table = output / 'native-matrix.tsv'
    if not table.is_file():
        raise RuntimeError(f'campaign failed before reporting: {output / "run.log"}')
    rows = [line.split('\t', 3) for line in table.read_text().splitlines()
            if line and not line.startswith('#')]
    if not rows or any(row[1] == 'snapshot' for row in rows):
        raise RuntimeError(f'no executable native control: {program}')
    # Any command error without a reported discrepancy is an infrastructure
    # failure, never evidence that the planted bug was detected.
    failures = {(row[2], row[3]) for row in rows if row[2] != 'agree'}
    if result.returncode and not failures:
        raise RuntimeError(f'campaign error without findings: {output / "run.log"}')
    return failures, len(rows)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--selection', required=True, type=Path)
    parser.add_argument('--output', required=True, type=Path)
    args = parser.parse_args()
    if subprocess.check_output(['git', 'status', '--porcelain'], cwd=ROOT, text=True).strip():
        parser.error('commit the lane changes before running transient mutations')
    selection = args.selection.resolve()
    selected = set(json.loads(selection.read_text())['selected'])
    for _, program, *_ in PLANTS:
        if program not in selected:
            parser.error(f'healthy cover omitted planted-bug control: {program}')
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    reports = []
    binary, archives = build()
    for program in sorted({plant[1] for plant in PLANTS}):
        found, count = matrix(binary, archives, selection, program,
                              output / ('healthy-' + Path(program).stem), True)
        if found:
            raise RuntimeError(f'healthy control already disagrees: {program}')
        reports.append(dict(control=program, healthy=True, configurations=count))
    for name, program, path, original, replacement in PLANTS:
        source = ROOT / path
        saved = source.read_bytes()
        text = saved.decode()
        if text.count(original) != 1:
            raise RuntimeError(f'{name}: mutation anchor must occur exactly once')
        try:
            source.write_text(text.replace(original, replacement))
            binary, archives = build()
            full, before = matrix(binary, archives, selection, program, output / name / 'exhaustive', True)
            slim, after = matrix(binary, archives, selection, program, output / name / 'slim', False)
            if not full or full != slim:
                raise RuntimeError(f'{name}: full={sorted(full)}, slim={sorted(slim)}')
            reports.append(dict(plant=name, detected=True, exhaustive=before, slim=after,
                                findings=sorted(full)))
        finally:
            source.write_bytes(saved)
    (output / 'replay.json').write_text(json.dumps(reports, indent=2) + '\n')
    print(json.dumps(reports, indent=2))


if __name__ == '__main__':
    main()
