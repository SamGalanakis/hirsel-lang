#!/usr/bin/env python3
"""Compile the existing fourteen benchmark ports at both optimization levels."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import struct
import time

PORTS = (
    'binarytrees', 'records-messages/scalar', 'saxpy-checked/integer',
    'slots-churn', 'closure-iter/scoped', 'closure-iter/erased',
    'resource-failure/throughput', 'resource-failure/heavy',
    'persistent-list-map/unique', 'persistent-list-map/shared',
    'batch-validation', 'expr-compiler', 'json-roundtrip', 'wordfreq',
)
CONFIGURATIONS = (
    'regions-on-whole', 'regions-on-singleton', 'regions-off-size-class',
)


def live_phases(pid):
    """Read this child's completed-call counters before terminating a timeout.

    Linux permits the direct parent to inspect its child. No signal handler or
    compiler instrumentation runs on the stalled check path. If access fails,
    preserve that failure rather than inventing zero LLVM time.
    """
    library = Path('/tmp/hirsel-compile-phases.so')
    symbols = subprocess.check_output(['nm', '-a', str(library)], text=True)
    offset = next(int(line.split()[0], 16) for line in symbols.splitlines()
                  if line.split()[-1:] == ['totals'])
    mappings = Path(f'/proc/{pid}/maps').read_text().splitlines()
    base = next(int(line.split('-')[0], 16) for line in mappings
                if line.endswith(str(library)) and line.split()[2] == '00000000')
    with open(f'/proc/{pid}/mem', 'rb', buffering=0) as memory:
        memory.seek(base + offset)
        values = struct.unpack('=4Q', memory.read(32))
    return dict(zip(('optimization_ns', 'emission_ns', 'optimization_calls',
                     'emission_calls'), values))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('helper', type=Path, help='already-built hirsel-native-build')
    args = parser.parse_args()
    helper = args.helper.resolve()
    bench = Path(__file__).resolve().parents[2]
    root = bench.parent.parent
    output = bench / 'out/compile-cost'
    scratch = bench / 'build/scalar-perf-compile'
    output.mkdir(parents=True, exist_ok=True)
    scratch.mkdir(parents=True, exist_ok=True)
    with (output / 'benchmark-compile-cost.jsonl').open('w') as log:
        for level in ('O2', 'O3'):
            for port in PORTS:
                source = bench / 'programs' / f'{port.replace("/", "-")}.hir'
                generated = scratch / source.name
                generated.write_text(source.read_text() + '\n' +
                                     (bench / 'programs/common.hir').read_text())
                for configuration in CONFIGURATIONS:
                    phase = scratch / 'phases.json'
                    binary = scratch / level / configuration / source.stem / 'program'
                    environment = os.environ.copy()
                    environment.update(
                        LD_PRELOAD='/tmp/hirsel-compile-phases.so',
                        HIRSEL_PERF_LEVEL=level,
                        HIRSEL_PERF_PHASES_OUTPUT=str(phase),
                        HIRSEL_BENCH_RUNTIME=str(scratch / 'runtime'),
                        CARGO_BUILD_JOBS='4', HIRSEL_JOBS='4', RUST_TEST_THREADS='4',
                    )
                    command = ['nice', '-n', '10', str(helper), str(generated),
                               str(binary), configuration]
                    phase.unlink(missing_ok=True)
                    start = time.monotonic()
                    process = subprocess.Popen(command, cwd=root, env=environment,
                                               stdout=subprocess.PIPE,
                                               stderr=subprocess.PIPE, text=True)
                    timeout = None
                    try:
                        stdout, stderr = process.communicate(timeout=300)
                    except subprocess.TimeoutExpired:
                        timeout = {'limit_seconds': 300,
                                   'phase_snapshot': 'completed LLVM calls before termination'}
                        try:
                            timeout['phases'] = live_phases(process.pid)
                        except (OSError, ValueError, StopIteration, struct.error) as error:
                            timeout['phase_read_error'] = repr(error)
                        process.terminate()
                        stdout, stderr = process.communicate(timeout=10)
                    row = dict(
                        actual_level=level, port=port, configuration=configuration,
                        command=command, returncode=process.returncode,
                        stderr=stderr, invocation_wall_seconds=time.monotonic() - start,
                        source_sha256=hashlib.sha256(generated.read_bytes()).hexdigest(),
                    )
                    if timeout is not None:
                        row['timeout'] = timeout
                    if process.returncode == 0:
                        row.update(json.loads(stdout))
                        row['phases'] = json.loads(phase.read_text())
                        assert row['phases']['optimization_calls']
                        assert row['phases']['emission_calls']
                    else:
                        row['stdout'] = stdout
                    log.write(json.dumps(row) + '\n')
                    log.flush()
                    print(level, port, configuration, process.returncode, flush=True)


if __name__ == '__main__':
    main()
