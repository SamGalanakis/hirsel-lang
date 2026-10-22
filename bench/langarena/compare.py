#!/usr/bin/env python3
"""Compare native Hirsel with a local Rust anchor and published LangArena results.

Only Hirsel and the upstream Rust port are built/run here. All other languages
come from the latest published results file. Cross-machine estimates multiply
local Hirsel/Rust ratios by published Rust values, never compare raw timings.
"""
import argparse
from datetime import datetime, timezone
import gzip
import hashlib
import json
import math
import os
from pathlib import Path
import random
import re
import shlex
import statistics
import subprocess
import sys
import time

import build

# Reuse the host's CPU31 convention and perf hooks, including SMT observations.

HERE, ROOT = build.HERE, build.ROOT
sys.path.insert(0, str(HERE.parent / 'micro'))
import measure  # noqa: E402
UPSTREAM = '3c9f84ddaeec834c0631747cb789e8cfd602f993'
METRICS = ('seconds', 'rss_kib', 'instructions', 'cycles')



def sha(path):
    with Path(path).open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def identity(path):
    path = Path(path).resolve()
    files = {str(p.relative_to(path)): sha(p) for p in sorted(path.rglob('*')) if p.is_file()} if path.is_dir() else {path.name: sha(path)}
    return dict(path=str(path), files=files,
                sha256=hashlib.sha256(json.dumps(files, sort_keys=True).encode()).hexdigest() if path.is_dir() else sha(path))


def sources_identity():
    paths = sorted(set([*HERE.glob('*.hir'), *HERE.glob('*.py'), HERE / 'run.js', HERE / 'test.js',
                        ROOT / 'bench/micro/measure.py',
                        *(p for p in (ROOT / 'stdlib').rglob('*') if p.is_file())]))
    files = {str(p.relative_to(ROOT)): sha(p) for p in paths}
    return dict(files=files, sha256=hashlib.sha256(json.dumps(files, sort_keys=True).encode()).hexdigest())


def source_metrics(files):
    content = b''.join(p.read_bytes() for p in sorted(files))
    return dict(bytes=len(content), gzip_bytes=len(gzip.compress(content, compresslevel=6, mtime=0)) if content else 0,
                lines=len(content.rstrip(b'\n').split(b'\n')) if content else 0,
                files=[str(p) for p in files])


def source_files(directory, suffix):
    return sorted(p for p in directory.rglob('*' + suffix) if p.is_file()
                  and not any(x in p.relative_to(directory).parts for x in ('build', 'target')))


def geometric(values):
    values = [v for v in values if v is not None and v > 0 and math.isfinite(v)]
    return math.exp(statistics.mean(math.log(v) for v in values)) if values else None


def parse_result(output, task):
    match = re.search(r'^' + re.escape(task) + r':\s+OK\s+in\s+([\d.]+)s\s*$', output, re.M)
    if not match:
        raise ValueError(f'{task}: no successful checksum line')
    return float(match[1])


def aggregate(entry, field, rounds):
    samples = entry.get('samples', [])
    if len(samples) != rounds or any(s['status'] != 'OK' for s in samples):
        return None
    values = [s[field] for s in samples if field in s]
    return statistics.median(values) if len(values) == rounds else None


def paired_ratio(row, metric, rounds):
    """Ratio of participant medians for a task with complete passing groups."""
    h = aggregate(row['hirsel'], metric, rounds)
    r = aggregate(row['rust'], metric, rounds)
    return h / r if h and r else None


def published_estimate(local_ratio, published_rust):
    if local_ratio is None or not isinstance(published_rust, (int, float)) or published_rust <= 0:
        return None
    return local_ratio * published_rust


def production_values(published, key):
    return {language: value for language, value in published.get(key, {}).items()
            if published['runs'].get(language) == 'prod'
            and isinstance(value, (int, float)) and value > 0}


def placement(estimate, values):
    if estimate is None or not values:
        return '—'
    return f'{1 + sum(value < estimate for value in values.values())} / {len(values) + 1}'


class Session:
    def __init__(self, args):
        self.args = args
        self.directory = HERE / 'build' / args.session
        self.directory.mkdir(parents=True, exist_ok=True)
        self.env = os.environ.copy()
        self.env.update(CARGO_BUILD_JOBS='2', RUSTUP_TOOLCHAIN='1.99.0', LC_ALL='C')
        self.commands = []
        self.perf = False

    def call(self, command, cwd=ROOT, label='command', check=True):
        log = self.directory / (re.sub(r'[^a-zA-Z0-9_.-]', '-', label) + '.log')
        timing = log.with_suffix('.elapsed')
        command = [str(x) for x in command]
        wrapped = ['nice', '-n', '10', str(ROOT / 'tools/capped'), '--timeout', str(self.args.timeout),
                   '/usr/bin/time', '-f', '%e', '-o', str(timing), *command]
        started = time.monotonic()
        with log.open('w') as out:
            result = subprocess.run(wrapped, cwd=cwd, env=self.env, stdout=out, stderr=subprocess.STDOUT)
        elapsed = time.monotonic() - started
        if timing.exists():
            try:
                elapsed = float(timing.read_text().splitlines()[-1])
            except ValueError:
                pass
        self.commands.append(dict(command=command, cwd=str(cwd), seconds=elapsed,
                                  code=result.returncode, log=str(log)))
        if check and result.returncode:
            tail = '\n'.join(log.read_text(errors='replace').splitlines()[-8:])
            raise RuntimeError(f'{label} (exit {result.returncode}): {tail}')
        return elapsed, result.returncode, log

    def reference(self):
        # Outside the Hirsel Cargo workspace, owned by this session: the
        # incremental measurement changes exactly one source file here.
        dest = self.args.tools.resolve() / ('reference-' + self.args.session)
        if dest.exists():
            raise ValueError(f'reference already exists: {dest}; use a fresh session')
        dest.parent.mkdir(parents=True, exist_ok=True)
        self.call(['git', 'clone', '--shared', self.args.upstream.resolve(), dest], label='clone-reference')
        self.call(['git', 'checkout', '--detach', UPSTREAM], cwd=dest, label='pin-reference')
        return dest

    def build_rust(self, reference):
        cwd = reference / 'rust'
        cargo = subprocess.check_output(['rustup', 'which', '--toolchain', '1.99.0', 'cargo'], text=True).strip()
        self.call([cargo, 'fetch', '--locked'], cwd, 'rust-dependencies')
        command = [cargo, 'build', '--release', '--locked', '-j2']
        consumed = identity(cwd / 'src')
        compiler = identity(subprocess.check_output(['rustup', 'which', '--toolchain', '1.99.0', 'rustc'], text=True).strip())
        cold, _, _ = self.call(command, cwd, 'rust-cold')
        marker = cwd / 'src/main.rs'
        original = marker.read_bytes()
        try:
            if b'RECOMPILE_MARKER_0' not in original:
                raise ValueError('upstream Rust incremental marker is missing')
            marker.write_bytes(original.replace(b'RECOMPILE_MARKER_0', b'RECOMPILE_MARKER_1'))
            incremental_source = identity(cwd / 'src')
            incremental, _, _ = self.call(command, cwd, 'rust-incremental')
        finally:
            marker.write_bytes(original)
        return dict(binary=str(cwd / 'target/release/benchmarks'), cwd=str(cwd), cold=cold, incremental=incremental,
                    build_command=command, source=source_metrics(source_files(cwd, '.rs')), status='ready',
                    cold_sources=consumed, incremental_sources=incremental_source,
                    compiler=compiler, binary_identity=identity(cwd / 'target/release/benchmarks'),
                    cargo_lock_sha256=sha(cwd / 'Cargo.lock'))

    def build_hirsel(self, debug=False):
        cli = build.compiler()
        native = build.native_environment()
        self.env.update({k: v for k, v in native.items() if k.startswith(('HIRSEL_', 'LLVM_', 'LD_LIBRARY'))})
        # A fresh session cache makes the first build cold and retains its
        # objects for the single-file incremental rebuild.
        self.env['HIRSEL_STD_OBJECTS'] = 'none'
        self.env['XDG_CACHE_HOME'] = str(self.directory / ('debug-cache' if debug else 'hirsel-cache'))
        manifest = build.generate()
        binary = self.directory / ('langarena-debug' if debug else 'langarena')
        label = 'cranelift-debug' if debug else 'hirsel'
        command = [str(cli), 'build', '--debug' if debug else '--release', str(manifest), '-o', str(binary)]
        consumed = sources_identity()
        compiler = identity(cli)
        runtime = {key: identity(native[key]) for key in ('LLVM_SYS_211_PREFIX', 'HIRSEL_RUNTIME_ARCHIVES', 'HIRSEL_LINK_KIT', 'HIRSEL_GLIBC_FLOOR')}
        cold, _, _ = self.call(command, label=label + '-cold')
        marker = HERE / 'main.hir'
        original = marker.read_bytes()
        try:
            marker.write_bytes(original.replace(b'RECOMPILE_MARKER_0', b'RECOMPILE_MARKER_1'))
            incremental_source = sources_identity()
            incremental, _, _ = self.call(command, label=label + '-incremental')
        finally:
            marker.write_bytes(original)
        return dict(binary=str(binary), cwd=str(ROOT), cold=cold, incremental=incremental,
                    build_command=command, source=source_metrics(source_files(HERE, '.hir')), status='ready',
                    cold_sources=consumed, incremental_sources=incremental_source,
                    compiler=compiler, runtime=runtime, binary_identity=identity(binary),
                    manifest_sha256=sha(manifest), grants_sha256=sha(HERE / 'build/grants.json'),
                    args=['--hirsel-grants', str(HERE / 'build/grants.json')])

    def sample(self, language, participant, task, config, index):
        label = f'{index}-{task}-{language}'
        rss = self.directory / (re.sub(r'[^a-zA-Z0-9_.-]', '-', label) + '.rss')
        counters = rss.with_suffix('.perf')
        before, after = rss.with_suffix('.before.stat'), rss.with_suffix('.after.stat')
        config_arg = os.path.relpath(config, ROOT) if language != 'rust' else str(config)
        command = ['/usr/bin/time', '-f', '%M', '-o', str(rss), participant['binary'],
                   *participant.get('args', []), config_arg, task]
        if self.perf:
            command = measure.perf_command(command, self.args.cpu, counters, before, after)
        else:
            command = ['taskset', '-c', str(self.args.cpu), *command]
        elapsed, code, log = self.call(command, Path(participant['cwd']), label, check=False)
        output = log.read_text(errors='replace')
        result = dict(exit=code, wall=elapsed, log=str(log))
        try:
            if code:
                raise ValueError(f'exit {code}: ' + ' '.join(output.splitlines()[-3:]))
            result['seconds'] = parse_result(output, task)
            result['rss_kib'] = int(rss.read_text().strip().splitlines()[-1])
            result['status'] = 'OK'
        except (ValueError, OSError) as error:
            result['status'] = 'failed: ' + str(error)
        if counters.exists():
            try:
                counts, running = measure.perf_counts(counters.read_text())
                result.update({event: counts[event] for event in METRICS if event in counts})
                result['counters'] = counts
                result['running_percent'] = running
                cpus = measure.siblings(self.args.cpu)
                result['sibling_before'] = measure.cpu_snapshot(cpus, before)
                result['sibling_after'] = measure.cpu_snapshot(cpus, after)
                result['sibling_busy_share'] = measure.busy_share(result['sibling_before'], result['sibling_after'])
            except (ValueError, OSError) as error:
                result['status'] = 'failed: ' + str(error)
        result['binary_sha256'] = participant['binary_identity']['sha256']
        return result


def blocked_tasks():
    result = {}
    for path in HERE.glob('*.hir'):
        for task, reason in re.findall(r'^// langarena-blocked: (\S+) \| (.+)$', path.read_text(), re.M):
            result[task] = reason
    return result


def display(value, digits=3):
    return '—' if value is None else f'{value:.{digits}f}'


def task_status(entry, rounds):
    samples = entry.get('samples', [])
    failures = [s['status'] for s in samples if s['status'] != 'OK']
    return failures[0] if failures else (f'OK ({len(samples)}/{rounds})' if samples else entry['status'])


def report(data, published, output):
    rounds = data['rounds']
    rows = data['tasks']
    debug = 'cranelift' in data['participants']
    full = data['config'] == 'run.js'
    eligible = {name: row for name, row in rows.items() if paired_ratio(row, 'seconds', rounds)}
    lines = ['# LangArena: Hirsel and a local Rust anchor', '',
             f"Config `{data['config']}`; {rounds} seeded interleaved rounds, seed {data['seed']}; {data['at']}.",
             f"Local host: {data['host']}. Hirsel: native LLVM release; Rust: upstream `cargo build --release`.",
             f"Upstream `{UPSTREAM}`. Published data: `{data['published_file']}`, SHA-256 `{data['published_sha256']}`.",
             f"Published host: {published.get('pc', 'unknown')}. Only Hirsel and Rust ran on this host.", '',
             'Runtime excludes warmup; process peak RSS includes preparation, warmup and runtime startup. '
             'Compiler and dependency downloads are excluded from compile measurements; cold Rust includes compiling fetched dependencies. '
             'Compile numbers are diagnostic singleton observations outside the ten-round timing session, with different whole-suite coverage; compile ratios and projections are ineligible. '
             'Incremental compilation changes RECOMPILE_MARKER_0 to RECOMPILE_MARKER_1 in one source file, as benchmarks.rb:2721-2733.',
             f"CPU {data['cpu']}, shared timing lock `{data['timing_lock']}` held for all interleaved rounds. Immutable consumed-source, compiler, native toolchain/runtime and binary SHA-256 identities were checked before/after the session.",
             f"Consumed source identity: `{data['consumed_sources']['sha256']}`.",
             f"Instruction counts are the verdict; cycles are supporting measurement. Available counters: {data['perf']}. Counters include startup and warmup. All local runs use nice -n 10 and tools/capped.", '',
             'Local ratios are median(Hirsel samples) / median(Rust samples), not medians of within-round ratios. '
             'Geometric means aggregate these per-task ratios of medians.', '',
             '## Local measurements', '',
             '| Task | Hirsel status | Rust status | Hirsel s | Rust s | Time H/R | Hirsel MiB | Rust MiB | RSS H/R | Instructions H/R |',
             '| --- | --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |']
    if debug:
        lines[-2] += ' Cranelift debug status | Debug s | Debug MiB | Debug/Rust time |'
        lines[-1] += ' --- | ---: | ---: | ---: |'
    for task, row in rows.items():
        h, r = row['hirsel'], row['rust']
        hs, rs = aggregate(h, 'seconds', rounds), aggregate(r, 'seconds', rounds)
        hm, rm = aggregate(h, 'rss_kib', rounds), aggregate(r, 'rss_kib', rounds)
        status = lambda e: task_status(e, rounds).replace('|', '/').replace('\n', ' ')[:160]
        runtime = lambda x: '<0.0005' if x == 0 else display(x, 9)
        lines.append(f'| {task} | {status(h)} | {status(r)} | {runtime(hs)} | {runtime(rs)} | '
                     f'{display(paired_ratio(row, "seconds", rounds))} | {display(hm / 1024 if hm else None)} | '
                     f'{display(rm / 1024 if rm else None)} | {display(paired_ratio(row, "rss_kib", rounds))} | '
                     f'{display(paired_ratio(row, "instructions", rounds))} |')
        if debug:
            d = row['cranelift']
            ds, dm = aggregate(d, 'seconds', rounds), aggregate(d, 'rss_kib', rounds)
            lines[-1] += (f' {status(d)} | {runtime(ds)} | {display(dm / 1024 if dm else None)} | '
                          f'{display(ds / rs if ds is not None and rs else None)} |')
    lines += ['', '## Geometric means and compile times', '',
              '| Metric | Hirsel / Rust | Paired tasks |', '| --- | ---: | ---: |']
    for field in METRICS:
        ratios = [ratio for row in rows.values() if (ratio := paired_ratio(row, field, rounds)) is not None]
        lines.append(f'| {field} | {display(geometric(ratios))} | {len(ratios)} / 50 |')
    lines += ['', '| Whole-suite compilation (diagnostic singleton) | Hirsel s | Local Rust s |',
              '| --- | ---: | ---: |']
    if debug:
        lines[-2] += ' Cranelift debug s |'
        lines[-1] += ' ---: |'
    for field in ('cold', 'incremental'):
        h, r = data['participants']['hirsel'].get(field), data['participants']['rust'].get(field)
        lines.append(f'| {field} | {display(h)} | {display(r)} |')
        if debug:
            lines[-1] += f' {display(data["participants"]["cranelift"].get(field))} |'
    lines += ['', 'These are single sequential build observations, outside the locked interleaved runtime session. '
              'They are ineligible for timing ratios, published-host projections or compile rankings. '
              'No additional compile campaign was run.', '']
    lines += ['', '## Position against published languages', '',
              'Estimate = local Hirsel/Rust × published Rust for the same metric and task. '
              'These are projections, not measurements on the published host. '
              'Runtime rank includes Hirsel and all available upstream production configurations; lower is faster. '
              'Hirsel/reference ratios below use only paired, passing tasks. '
              'No full-suite ranking is claimed while tasks are missing.', '']
    if not full:
        lines += ['Quick mode runs test.js. Its workloads differ from the published run.js, so no published runtime, memory or rank estimates are made.', '']
    lines += ['| Task | Estimated s | Estimated MiB | Runtime rank | H / C/Clang | H / Zig | H / Goose/Clang |',
              '| --- | ---: | ---: | ---: | ---: | ---: | ---: |']
    for task, row in rows.items():
        values = production_values(published, task + '-runtime')
        estimate = published_estimate(paired_ratio(row, 'seconds', rounds), values.get('Rust')) if full else None
        memory = published_estimate(paired_ratio(row, 'rss_kib', rounds),
                                    published.get(task + '-mem-mb', {}).get('Rust')) if full else None
        ratios = [estimate / values[lang] if estimate and values.get(lang) else None
                  for lang in ('C/Clang', 'Zig', 'Goose/Clang')]
        lines.append(f'| {task} | {display(estimate)} | {display(memory)} | {placement(estimate, values)} | '
                     + ' | '.join(display(r) for r in ratios) + ' |')
    lines += ['', '| Published production configuration | Geomean estimated H/reference time | Paired tasks | '
              'Geomean estimated H/reference RSS |',
              '| --- | ---: | ---: | ---: |']
    for lang in sorted(k for k, group in published['runs'].items() if group == 'prod'):
        time_ratios, rss_ratios = [], []
        if full:
            for task, row in rows.items():
                for metric, suffix, collector in [('seconds', '-runtime', time_ratios), ('rss_kib', '-mem-mb', rss_ratios)]:
                    vals = published.get(task + suffix, {})
                    estimate = published_estimate(paired_ratio(row, metric, rounds), vals.get('Rust'))
                    if estimate and isinstance(vals.get(lang), (int, float)) and vals[lang] > 0:
                        collector.append(estimate / vals[lang])
        lines.append(f'| {lang} | {display(geometric(time_ratios))} | {len(time_ratios)} / 50 | '
                     f'{display(geometric(rss_ratios))} |')
    lines += ['', f'Paired local runtime coverage: {len(eligible)} / 50. '
              'Printed upstream runtime zeros are below its millisecond resolution and excluded from runtime ratios.', '',
              '## Source / expressiveness', '',
              'Upstream benchmarks.rb:54-159 concatenates source files, including comments, then measures bytes, gzip bytes and lines. '
              'docs/gen.rb:280-321 defines boilerplate = source/gzip and the expressiveness score as '
              '(boilerplate / mean boilerplate) × (lines / mean lines)^1.5. '
              'Hirsel implements a subset, so its full-suite expressiveness rank is not comparable. '
              'Per-task entries measure the owning group file; shared files repeat here but count once in suite totals.', '',
              '| Task | Hirsel bytes | Gzip bytes | Lines | Local Rust bytes | Gzip bytes | Lines |',
              '| --- | ---: | ---: | ---: | ---: | ---: | ---: |']
    for task, row in rows.items():
        h, r = row['hirsel']['source'], row['rust']['source']
        lines.append(f'| {task} | {h["bytes"]} | {h["gzip_bytes"]} | {h["lines"]} | {r["bytes"]} | {r["gzip_bytes"]} | {r["lines"]} |')
    lines += ['', '| Whole-suite source | Bytes | Gzip bytes | Lines | Source/gzip |', '| --- | ---: | ---: | ---: | ---: |']
    for lang in ('hirsel', 'rust'):
        s = data['participants'][lang].get('source', {})
        lines.append(f'| Local {lang} | {s.get("bytes", "—")} | {s.get("gzip_bytes", "—")} | {s.get("lines", "—")} | '
                     f'{display(s["bytes"] / s["gzip_bytes"] if s.get("gzip_bytes") else None)} |')
    for lang, size in published.get('source-size-kb', {}).items():
        compressed = published['source-gzip-size-kb'].get(lang)
        lines.append(f'| Published {lang} | {display(size * 1024, 0)} | {display(compressed * 1024 if compressed else None, 0)} | '
                     f'{published["source-lines-count"].get(lang, "—")} | {display(size / compressed if compressed else None)} |')
    lines += ['', '## Missing and blocked', '']
    for lang, participant in data['participants'].items():
        if participant['status'] != 'ready':
            lines.append(f'- Local {lang}: {participant["status"]}')
    for task, row in rows.items():
        if row['hirsel']['status'] != 'ready':
            lines.append(f'- {task}: {row["hirsel"]["status"]}.')
    lines += ['', '## Reproduce', '', '```sh', data['invocation'], '```', '', 'Local build commands:', '']
    for lang, participant in data['participants'].items():
        if participant.get('build_command'):
            lines.append(f'- {lang}: `{shlex.join(participant["build_command"])}`')
    lines += ['', 'Raw samples, counters, logs and build commands are in ignored `build/' + data['session'] + '/measurements.json`.', '']
    lines += ['', '## Binary identities', '', '| Participant | Compiler SHA-256 | Executable SHA-256 |', '| --- | --- | --- |']
    for lang, p in data['participants'].items():
        if p['status'] == 'ready':
            lines.append(f"| {lang} | {p['compiler']['sha256']} | {p['binary_identity']['sha256']} |")
    output.write_text('\n'.join(lines) + '\n')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--debug', action='store_true', help='include native Cranelift debug as a separate participant')
    parser.add_argument('--quick', action='store_true', help='test.js; correctness only, no published placement')
    parser.add_argument('--filter', action='append', default=[], help='case-insensitive substring; repeat for OR')
    parser.add_argument('--upstream', type=Path, default=Path('/tmp/ref-langarena'))
    parser.add_argument('--published', type=Path, help='defaults to the latest results/*.js in upstream')
    parser.add_argument('--tools', type=Path, default=Path('/workspace/tmp/langarena-tools'))
    parser.add_argument('--rounds', type=int, default=10)
    parser.add_argument('--cpu', type=int, default=31)
    parser.add_argument('--seed', type=int, default=20261021)
    parser.add_argument('--timeout', type=int, default=900)
    parser.add_argument('--session', default=datetime.now(timezone.utc).strftime('%Y%m%dT%H%M%S'))
    parser.add_argument('--output', type=Path, help='report path; default RESULTS-<session>.md')
    args = parser.parse_args()
    if args.rounds < 1 or not 1 <= args.timeout <= 900:
        parser.error('rounds must be positive; timeout must be between 1 and 900')
    if not re.fullmatch(r'[a-zA-Z0-9_.-]+', args.session):
        parser.error('session must be a simple directory name')
    if args.output is None:
        args.output = HERE / f'RESULTS-{args.session}.md'
    session = Session(args)
    raw = session.directory / 'measurements.json'
    if raw.exists():
        parser.error('session already measured; use a fresh session for a cold build')
    published_file = args.published or sorted((args.upstream / 'results').glob('*.js'))[-1]
    published = json.loads(published_file.read_text())
    reference = session.reference()
    config = HERE / ('test.js' if args.quick else 'run.js')
    tasks = json.loads(config.read_text())
    modules = build.groups()
    implemented = {name: module for module, names in modules.items() for name in names}
    blocked = blocked_tasks()
    data = dict(config=config.name, rounds=args.rounds, seed=args.seed, session=args.session, cpu=args.cpu, timing_lock=str(measure.TIMING_LOCK), status='running',
                consumed_sources=sources_identity(),
                at=datetime.now(timezone.utc).isoformat(), host=' '.join(os.uname()),
                invocation=shlex.join(['python3', 'bench/langarena/compare.py', *sys.argv[1:]]),
                hirsel_commit=subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip(),
                config_sha256=hashlib.sha256(config.read_bytes()).hexdigest(),
                published_file=published_file.name, published_sha256=hashlib.sha256(published_file.read_bytes()).hexdigest(),
                upstream=UPSTREAM, participants={}, tasks={}, perf='unavailable')
    for language in (('hirsel', 'rust', 'cranelift') if args.debug else ('hirsel', 'rust')):
        print('building', language, flush=True)
        try:
            data['participants'][language] = session.build_rust(reference) if language == 'rust' else session.build_hirsel(debug=language == 'cranelift')
        except (RuntimeError, OSError, subprocess.CalledProcessError) as error:
            data['participants'][language] = dict(status='blocked: ' + str(error).replace('\n', ' ')[:800])
    _, status, _ = session.call(['taskset', '-c', str(args.cpu), 'perf', 'stat', '-e', 'cycles,instructions', '--', 'true'], label='perf-probe', check=False)
    session.perf = status == 0
    data['perf'] = 'cycles + instructions' if session.perf else 'blocked: host denies perf stat cycles,instructions (see perf-probe.log)'
    jobs = []
    for task in tasks:
        name = task['name']
        selected = not args.filter or any(f.casefold() in name.casefold() for f in args.filter)
        row = data['tasks'][name] = {}
        for language, participant in data['participants'].items():
            files = []
            if language != 'rust' and name in implemented:
                files = [HERE / (implemented[name] + '.hir')]
            if language == 'rust':
                stem = name.split('::')[-1].lower() if name.startswith(('CLBG::', 'Etc::')) else name.split('::')[0].lower()
                files = [p for p in source_files(reference / 'rust/src/benchmarks', '.rs')
                         if p.stem == stem or p.stem.startswith(stem + '_')]
            entry = row[language] = dict(source=source_metrics(files), samples=[])
            if name not in implemented:
                entry['status'] = ('blocked: ' + blocked[name] if name in blocked else 'missing: unported task') if language != 'rust' else 'not measured: no Hirsel port'
            elif participant['status'] != 'ready':
                entry['status'] = 'blocked: build unavailable'
            elif not selected:
                entry['status'] = 'not selected'
            else:
                entry['status'] = 'ready'
                jobs.append((name, language))
    def verify_frozen():
        if sources_identity() != data['consumed_sources']:
            raise RuntimeError('consumed Hirsel sources changed during the session')
        for language, participant in data['participants'].items():
            if participant['status'] != 'ready':
                continue
            if identity(participant['binary']) != participant['binary_identity']:
                raise RuntimeError(f'{language} binary changed during the session')
            if language != 'rust' and (sha(HERE / 'build/hirsel.json') != participant['manifest_sha256']
                                         or sha(HERE / 'build/grants.json') != participant['grants_sha256']):
                raise RuntimeError('consumed manifest/grants changed during the session')
            if language == 'rust' and identity(reference / 'rust/src') != participant['cold_sources']:
                raise RuntimeError('consumed Rust sources changed during the session')
    rng = random.Random(args.seed)
    raw.write_text(json.dumps(data | {'commands': session.commands}, indent=2) + '\n')
    print(f'waiting for CPU {args.cpu} timing lock', flush=True)
    with measure.timing_lock(args.cpu):
        data['lock_acquired_at'] = datetime.now(timezone.utc).isoformat()
        verify_frozen()
        for index in range(args.rounds):
            rng.shuffle(jobs)
            print(f'round {index + 1}/{args.rounds}: {len(jobs)} runs', flush=True)
            for task, language in jobs:
                sample = session.sample(language, data['participants'][language], task, config, index)
                data['tasks'][task][language]['samples'].append(sample)
                if sample['status'] != 'OK':
                    print(language, task, sample['status'], flush=True)
                # Save each completed sample, so interruption cannot erase evidence.
                data['commands'] = session.commands
                raw.write_text(json.dumps(data, indent=2) + '\n')
            verify_frozen()
        data['lock_released_at'] = datetime.now(timezone.utc).isoformat()
    for participant in data['participants'].values():
        if participant['status'] != 'ready':
            continue
        if identity(participant['compiler']['path']) != participant['compiler']:
            raise RuntimeError('consumed compiler changed during the session')
        for recorded in participant.get('runtime', {}).values():
            if identity(recorded['path']) != recorded:
                raise RuntimeError('consumed native toolchain/runtime changed during the session')
    data['status'] = 'complete'
    data['commands'] = session.commands
    raw.write_text(json.dumps(data, indent=2) + '\n')
    report(data, published, args.output)
    print(args.output)
    return 1 if any(p['status'] != 'ready' for p in data['participants'].values()) or any(
        s['status'] != 'OK' for languages in data['tasks'].values() for entry in languages.values() for s in entry['samples']) else 0


if __name__ == '__main__':
    sys.exit(main())
