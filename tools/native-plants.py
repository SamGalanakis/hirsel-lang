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


def build(history=False):
    targets = ['//:hirsel-conformance-bin-native', '//:hirsel-rt-archives']
    if history:
        targets.append('//:hirsel-conformance-bin-native-select')
    targets += ['//tools/buck2/native-toolchain:' + name
                for name in ['llvm', 'arm', 'qemu', 'valgrind']]
    command = [str(BUCK), 'build', '--target-platforms', '//tools/buck2:release',
               *targets, '--show-full-json-output']
    result = subprocess.run(command, cwd=ROOT, check=True, text=True, stdout=subprocess.PIPE)
    outputs = {target.rsplit(':', 1)[-1]: str(Path(path).resolve())
               for target, path in json.loads(result.stdout).items()}
    llvm, arm, qemu, valgrind = [Path(outputs[name]) for name in ['llvm', 'arm', 'qemu', 'valgrind']]
    # Match the explicit native inputs exported by test_launcher.sh.
    env = {
        'LLVM_SYS_211_PREFIX': str(llvm),
        'LD_LIBRARY_PATH': str(llvm / 'lib'),
        'HIRSEL_AARCH64_CC': str(arm / 'bin/aarch64-none-linux-gnu-gcc'),
        'HIRSEL_AARCH64_SYSROOT': str(arm / 'aarch64-none-linux-gnu/libc'),
        'HIRSEL_QEMU_AARCH64': str(qemu / 'qemu-aarch64'),
        'HIRSEL_VALGRIND': str(valgrind / 'bin/valgrind'),
        'VALGRIND_LIB': str(valgrind / 'libexec/valgrind'),
        'HIRSEL_VALGRIND_RUNTIME': str(valgrind / 'lib/x86_64-linux-gnu'),
    }
    return outputs, env


def matrix(artifacts, tools, selection, program, output, exhaustive):
    env = os.environ | tools | {
        'HIRSEL_RUNTIME_ARCHIVES': str(Path(artifacts['hirsel-rt-archives']).resolve()),
        'HIRSEL_NATIVE_SELECTION': str(selection),
    }
    binary = artifacts['hirsel-conformance-bin-native']
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
    parser.add_argument('--history-only', action='store_true',
                        help='replay the pinned historical set instead of transient faults')
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
    artifacts, tools = build(history=args.history_only)
    if args.history_only:
        env = os.environ | tools | {'HIRSEL_RUNTIME_ARCHIVES': artifacts['hirsel-rt-archives']}
        command = ['flock', LOCK, artifacts['hirsel-conformance-bin-native-select'],
                   '--root', str(ROOT), '--replay-history', '--seed', '1']
        with (output / 'history.json').open('w') as log:
            subprocess.run(command, cwd=ROOT, env=env, stdout=log, check=True)
        print((output / 'history.json').read_text())
        return
    for program in sorted({plant[1] for plant in PLANTS}):
        found, count = matrix(artifacts, tools, selection, program,
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
            artifacts, tools = build()
            full, before = matrix(artifacts, tools, selection, program, output / name / 'exhaustive', True)
            slim, after = matrix(artifacts, tools, selection, program, output / name / 'slim', False)
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
