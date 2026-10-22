#!/usr/bin/env python3
"""Checker cost growth at N=10,40,160 (run by //:gate-checker-scaling-shard-<i>).

Each cell is one `hirsel check` of a generated program. Instructions are
Cachegrind Ir under the pinned Valgrind (deterministic, so one run decides);
peak RSS comes from a separate uninstrumented run. Wall time is printed for
diagnosis only and never judged. Growth budget: the marginal cost over an
empty-program baseline may grow at most TOLERANCE times N log2(N) between
consecutive sizes; RSS additionally permits RSS_NOISE_KB of allocator noise.
A cell that exceeds its memory cap or timeout is a bounded failure: a finding,
never a reason to raise the cap.
"""
import argparse
import json
import math
import os
from pathlib import Path
import re
import resource
import signal
import subprocess
import tempfile
import time

SIZES = (10, 40, 160)
FAMILIES = ('match_try_fields', 'record_fields', 'nested_loops_try', 'closure_captures', 'if_chain', 'deep_nesting')
# Families expected to fail, each with the owner who removes it after the fix.
# match_try_fields was GAP 5 (18-fold Ir from N=40 to 160) until 19aa5a01.
EXPECTED = {}
TOLERANCE = 2.0
RSS_NOISE_KB = 16 * 1024
DATA_CAP = 2 * 1024**3
RSS_TIMEOUT = 45
IR_TIMEOUT = 240


def program(family, n):
    next_fn = 'fn next(mut x:u64)->Result[u64,String] uses {} { x += 1\n Ok(x) }\n'
    fields = ','.join(f'f{i}:u64' for i in range(n))
    if family == 'match_try_fields':
        init = ','.join(f'f{i}:0u64' for i in range(n))
        arms = ','.join(f'{i}u64 => {{ r.f{i} = next(mut pos)? }}' for i in range(n))
        return next_fn + f'type R {{{fields}}}\nfn probe(count:u64)->Result[R,String] uses {{}} {{ let mut r=R({init})\nlet mut pos:u64=0\nwhile pos<count {{ match pos % {n}u64 {{{arms}, _ => ()}} }}\nOk(r) }}\n'
    if family == 'record_fields':
        return f'type R {{{fields}}}\nfn probe(r:R)->u64 {{ r.f{n-1} }}\n'
    if family == 'nested_loops_try':
        return next_fn + 'fn probe(mut x:u64)->Result[u64,String] uses {} {' + 'while x < 1000u64 {' * n + 'x = next(mut x)?' + '}' * n + '\nOk(x)}\n'
    if family == 'closure_captures':
        bindings = '\n'.join(f'let x{i}=a' for i in range(n))
        return f'fn probe(a:u64)->u64{{{bindings}\nlet f=fn()->u64{{' + '+'.join(f'x{i}' for i in range(n)) + '}\nf()}\n'
    if family == 'if_chain':
        return 'fn probe(a:u64)->u64{' + ''.join(f'if a=={i}u64 {{{i}u64}} else ' for i in range(n)) + '{a}}\n'
    if family == 'deep_nesting':
        return 'fn probe(a:u64)->u64{' + '{' * n + 'a' + '}' * n + '}\n'
    raise ValueError(family)


def environment():
    env = {key: value for key, value in os.environ.items() if key in (
        'PATH', 'LLVM_SYS_231_PREFIX', 'HIRSEL_IMAGE', 'HIRSEL_VALGRIND', 'VALGRIND_LIB',
        'HIRSEL_VALGRIND_RUNTIME', 'LD_LIBRARY_PATH')}
    env.update(LC_ALL='C', HIRSEL_JOBS='1', HIRSEL_MEMORY_LIMIT=str(DATA_CAP))
    return env


def capped():
    soft, hard = resource.getrlimit(resource.RLIMIT_DATA)
    cap = DATA_CAP if soft == resource.RLIM_INFINITY else min(DATA_CAP, soft)
    resource.setrlimit(resource.RLIMIT_DATA, (cap, hard))


def bounded(command, env, timeout):
    """(status, peak RSS KiB, seconds, stderr tail) of one process group."""
    started = time.monotonic()
    with tempfile.TemporaryFile() as err:
        process = subprocess.Popen([str(x) for x in command], env=env, stdout=subprocess.DEVNULL, stderr=err,
                                   preexec_fn=capped, start_new_session=True)
        deadline = started + timeout
        while True:
            pid, status, usage = os.wait4(process.pid, os.WNOHANG)
            if pid:
                code = os.waitstatus_to_exitcode(status)
                break
            if time.monotonic() > deadline:
                os.killpg(process.pid, signal.SIGKILL)
                _, _, usage = os.wait4(process.pid, 0)
                code = 124
                break
            time.sleep(0.02)
        err.seek(0)
        detail = err.read()[-2000:].decode(errors='replace')
    if code == 124:
        detail = f'exceeded the {timeout} s budget'
    return code, usage.ru_maxrss, time.monotonic() - started, detail


def valgrind(env):
    if 'HIRSEL_VALGRIND' in env:
        # Pin the loader/libc as well as Valgrind (as tools/perf-counts does).
        runtime = env['HIRSEL_VALGRIND_RUNTIME']
        libraries = runtime + (':' + env['LLVM_SYS_231_PREFIX'] + '/lib' if 'LLVM_SYS_231_PREFIX' in env else '')
        return [env['HIRSEL_VALGRIND']], [runtime + '/ld-linux-x86-64.so.2', '--library-path', libraries]
    return ['valgrind'], []


def instructions(text):
    events = re.search(r'^events: (.+)$', text, re.M)
    summary = re.search(r'^summary: (.+)$', text, re.M)
    if not events or not summary:
        raise ValueError('incomplete Cachegrind output')
    return int(summary[1].split()[events[1].split().index('Ir')])


def sample(compiler, source, directory, env):
    command = [compiler, 'check', source]
    status, rss, seconds, detail = bounded(command, env, RSS_TIMEOUT)
    row = {'status': status, 'rss_kb': rss, 'seconds': round(seconds, 3), 'instructions': None, 'detail': detail}
    if status != 0:
        return row
    output = directory / 'cachegrind.out'
    output.unlink(missing_ok=True)
    tool, launch = valgrind(env)
    status, _, seconds, detail = bounded([*tool, '--tool=cachegrind', '--cache-sim=no', '--branch-sim=no',
                                          f'--cachegrind-out-file={output}', *launch, *command], env, IR_TIMEOUT)
    row['ir_seconds'] = round(seconds, 3)
    if status != 0:
        return row | {'status': status, 'detail': 'under Cachegrind: ' + detail}
    return row | {'instructions': instructions(output.read_text())}


def judge(samples, baseline):
    issues = []
    for metric in ('instructions', 'rss_kb'):
        base = baseline[metric]
        values = {n: samples[n][metric] for n in SIZES if samples[n]['status'] == 0 and samples[n][metric] is not None}
        for small, large in zip(SIZES, SIZES[1:]):
            if small not in values or large not in values:
                continue
            ratio = large * math.log2(large) / (small * math.log2(small))
            noise = RSS_NOISE_KB if metric == 'rss_kb' else 0
            allowed = max(values[small] - base, 1) * ratio * TOLERANCE + noise
            if values[large] - base > allowed:
                issues.append(f'{metric} {small}->{large}: marginal {values[large]-base:.0f} exceeds {allowed:.0f} '
                              f'({TOLERANCE:g} x N log N + {noise} noise)')
    return issues


def verdict(family, samples, baseline):
    """(passed, line) for one family's cells."""
    errors = [f'N={n}: bounded checker failure ({row["status"]}): {row["detail"]}'
              for n in SIZES if (row := samples[n])['status'] != 0]
    errors += judge(samples, baseline)
    expected = EXPECTED.get(family)
    if not expected:
        return not errors, f'{"FAIL" if errors else "PASS"} {family}' + ''.join('; ' + e for e in errors)
    if not errors:
        return False, f'XPASS {family}: remove the expected failure ({expected})'
    # A generator's own syntax or type error is never the expected growth
    # failure: only growth, the memory cap or the timeout count.
    growth = all(row['status'] in (0, 124, -6, -9) or 'memory limit' in row['detail'] for row in samples.values())
    return growth, f'{"XFAIL" if growth else "FAIL"} {family}: {expected}; ' + '; '.join(errors)


def run(compiler, families):
    env = environment()
    started = time.monotonic()
    with tempfile.TemporaryDirectory(prefix='checker-scaling-') as temporary:
        directory = Path(temporary)
        empty = directory / 'empty.hir'
        empty.write_text('fn baseline()->u64{0u64}\n')
        baseline = sample(compiler, empty, directory, env)
        print(json.dumps({'family': 'baseline', 'n': 0, **baseline}), flush=True)
        if baseline['status'] != 0:
            raise ValueError('baseline check failed: ' + baseline['detail'])
        ok = True
        for family in families:
            samples = {}
            for n in SIZES:
                source = directory / f'{family}-{n}.hir'
                source.write_text(program(family, n))
                samples[n] = sample(compiler, source, directory, env)
                print(json.dumps({'family': family, 'n': n, **samples[n]}), flush=True)
            passed, line = verdict(family, samples, baseline)
            print(line, flush=True)
            ok &= passed
    usage = resource.getrusage(resource.RUSAGE_CHILDREN)
    print(f'checker scaling: {len(families)} families; {time.monotonic() - started:.2f} seconds; '
          f'{usage.ru_utime + usage.ru_stime:.2f} CPU seconds', flush=True)
    return ok


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--hirsel', type=Path, required=True)
    parser.add_argument('--family', choices=FAMILIES, action='append')
    args = parser.parse_args()
    return 0 if run(args.hirsel.resolve(), args.family or FAMILIES) else 1


if __name__ == '__main__':
    try:
        raise SystemExit(main())
    except (ValueError, OSError) as error:
        raise SystemExit(f'checker scaling: {error}')
