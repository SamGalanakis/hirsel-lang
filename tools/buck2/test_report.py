#!/usr/bin/env python3
"""Return test progress before the RE action's 600-second deadline.

Libtest events follow rust-lang/rust library/test/src/formatters/json.rs:36,
121-177 (a7c37f9f). Keep the usual result lines for counts and cost extraction.
The 5% reporting reserve is inside the action cap; it never extends a test.
"""
import json
import ctypes
import os
from pathlib import Path
import re
import selectors
import shutil
import signal
import subprocess
import sys
import time

ACTION_TIMEOUT_SECONDS = 600
INNER_DEADLINE_SECONDS = ACTION_TIMEOUT_SECONDS * 0.95
STACK_DUMP_SECONDS = 20


def owned_descendants():
    # Include adopted orphans and children created by any worker thread.
    pending = [os.getpid()]
    owned = set()
    while pending:
        parent = pending.pop()
        for task in Path(f'/proc/{parent}/task').glob('*'):
            try:
                children = (task / 'children').read_text().split()
            except OSError:
                continue
            for child in map(int, children):
                if child not in owned:
                    owned.add(child)
                    pending.append(child)
    return sorted(owned)


def dump_owned_stacks():
    # Spend only the existing reporting reserve, even if a debugger hangs.
    end = time.monotonic() + STACK_DUMP_SECONDS
    debugger = shutil.which('gdb') or shutil.which('eu-stack')
    for pid in owned_descendants():
        if time.monotonic() >= end:
            print('HIRSEL_TEST_STACKS reporting budget exhausted', file=sys.stderr, flush=True)
            break
        print(f'HIRSEL_TEST_STACKS pid={pid} debugger={debugger or "unavailable"}',
              file=sys.stderr, flush=True)
        # Keep useful thread/wait information when the action has no debugger
        # or its sandbox denies ptrace. Never inspect unrelated processes.
        for task in sorted(Path(f'/proc/{pid}/task').glob('*')):
            if time.monotonic() >= end:
                break
            fields = {}
            for name in ('comm', 'wchan', 'stack'):
                try:
                    fields[name] = (task / name).read_text().strip()
                except OSError as error:
                    fields[name] = str(error)
            print(f'HIRSEL_TEST_THREAD tid={task.name} {json.dumps(fields)}',
                  file=sys.stderr, flush=True)
        if debugger:
            command = ([debugger, '-q', '-nx', '-batch', '-p', str(pid),
                        '-ex', 'set pagination off', '-ex', 'thread apply all bt']
                       if Path(debugger).name == 'gdb' else [debugger, '-p', str(pid)])
            remaining = end - time.monotonic()
            if remaining <= 0:
                break
            try:
                result = subprocess.run(command, stdin=subprocess.DEVNULL,
                                        stdout=sys.stderr, stderr=sys.stderr,
                                        timeout=min(5, remaining))
                print(f'HIRSEL_TEST_STACKS pid={pid} exit={result.returncode}',
                      file=sys.stderr, flush=True)
            except (OSError, subprocess.TimeoutExpired) as error:
                print(f'HIRSEL_TEST_STACKS pid={pid} {error}', file=sys.stderr, flush=True)


def reap_owned_children(child):
    # PR_SET_CHILD_SUBREAPER reparents orphaned descendants here (Linux
    # prctl(2)). Direct children cannot reuse their PID until we reap them.
    # Kill one generation, reap it, then handle its newly adopted children.
    # Preserve the inherited PGID: NativeLink samples/enforces that group.
    child.poll()
    children_file = Path(f'/proc/self/task/{os.getpid()}/children')
    while children := children_file.read_text().split():
        for pid in children:
            try:
                os.kill(int(pid), signal.SIGKILL)
            except ProcessLookupError:
                pass
        if child.returncode is None:
            child.wait()
        for pid in children:
            try:
                # Reap only the children just killed, never a newly adopted
                # (and not yet signalled) grandchild that could still be live.
                os.waitpid(int(pid), 0)
            except ChildProcessError:
                pass


class Progress:
    def __init__(self, raw_json=False):
        self.running = {}
        self.finished = {}
        self.raw_json = raw_json

    def unittest_line(self, line):
        # unittest's verbose runner writes the start before calling the body,
        # without a newline. Observe that partial line as well as its verdict.
        match = re.match(rb'\S+ \(([^)]+)\) \.\.\. (.*)', line, re.S)
        if match:
            name, result = (part.decode(errors='replace').strip() for part in match.groups())
            if result in ('ok', 'FAIL', 'ERROR') or result.startswith('skipped '):
                self.running.pop(name, None)
                self.finished[name] = {'ok': 'ok', 'FAIL': 'failed', 'ERROR': 'failed'}.get(result, 'ignored')
            elif name not in self.finished:
                self.running.setdefault(name, time.monotonic())

    def line(self, line):
        try:
            event = json.loads(line)
        except (ValueError, UnicodeError):
            event = None
        if not isinstance(event, dict) or event.get('type') not in ('test', 'suite'):
            sys.stdout.buffer.write(line)
            sys.stdout.buffer.flush()
            return
        kind, status = event['type'], event.get('event')
        if kind == 'test':
            name = event['name']
            if status == 'started':
                self.running[name] = time.monotonic()
            elif status in ('ok', 'failed', 'ignored'):
                self.running.pop(name, None)
                self.finished[name] = status
        if self.raw_json:
            sys.stdout.buffer.write(line)
            sys.stdout.buffer.flush()
        elif kind == 'test':
            if status == 'started':
                print(f'HIRSEL_TEST_STARTED {name}', flush=True)
            elif status in ('ok', 'failed', 'ignored'):
                timing = f" <{event['exec_time']:.6f}s>" if 'exec_time' in event else ''
                print(f"test {name} ... {'FAILED' if status == 'failed' else status}{timing}", flush=True)
                if event.get('stdout'):
                    print(event['stdout'], end='', flush=True)
                if event.get('message'):
                    print(event['message'], flush=True)
        elif status == 'started':
            print(f"running {event['test_count']} tests", flush=True)
        elif status in ('ok', 'failed'):
            print(f"test result: {'ok' if status == 'ok' else 'FAILED'}. "
                  f"{event['passed']} passed; {event['failed']} failed; "
                  f"{event['ignored']} ignored; {event.get('measured', 0)} measured; "
                  f"{event.get('filtered_out', 0)} filtered out; "
                  f"finished in {event.get('exec_time', 0):.2f}s", flush=True)

    def interrupted(self, reason):
        print(f'HIRSEL_TEST_INNER_DEADLINE {reason}; completed={len(self.finished)} '
              f'running={len(self.running)}; action cap={ACTION_TIMEOUT_SECONDS}s',
              file=sys.stderr, flush=True)
        for name, status in sorted(self.finished.items()):
            print(f'HIRSEL_TEST_FINISHED {status} {name}', file=sys.stderr, flush=True)
        for name, started in sorted(self.running.items()):
            print(f'HIRSEL_TEST_RUNNING {name} elapsed={time.monotonic() - started:.3f}s',
                  file=sys.stderr, flush=True)
        if not self.running:
            print('HIRSEL_TEST_RUNNING <launcher setup, discovery, or child teardown>',
                  file=sys.stderr, flush=True)


def supervise(command, *, deadline=INNER_DEADLINE_SECONDS, raw_json=False):
    progress = Progress(raw_json)
    stopped = None

    def stop(signum, _frame):
        nonlocal stopped
        stopped = signum

    previous = {sig: signal.signal(sig, stop) for sig in (signal.SIGTERM, signal.SIGINT, signal.SIGHUP)}
    end = time.monotonic() + deadline
    child = None
    # Adopt orphaned descendants so cleanup needs only our direct children,
    # without a new session/group escaping the executor's resource enforcement.
    libc = ctypes.CDLL(None, use_errno=True)
    previous_subreaper = ctypes.c_int()
    if libc.prctl(37, ctypes.byref(previous_subreaper), 0, 0, 0) != 0 or libc.prctl(36, 1, 0, 0, 0) != 0:
        raise OSError(ctypes.get_errno(), 'cannot establish test descendant ownership')
    try:
        child = subprocess.Popen(command, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        with selectors.DefaultSelector() as selector:
            selector.register(child.stdout, selectors.EVENT_READ)
            selector.register(child.stderr, selectors.EVENT_READ)
            pending = {child.stdout: b'', child.stderr: b''}
            while selector.get_map() or child.poll() is None:
                remaining = end - time.monotonic()
                if stopped or remaining <= 0:
                    progress.interrupted(f'signal={stopped}' if stopped else f'after {deadline:g}s')
                    if not stopped:
                        dump_owned_stacks()
                    return 128 + stopped if stopped else 124
                for key, _ in selector.select(min(remaining, 0.1)):
                    stream = key.fileobj
                    chunk = os.read(stream.fileno(), 65536)
                    if not chunk:
                        selector.unregister(stream)
                        if pending[stream] and stream is child.stdout:
                            progress.line(pending[stream])
                        pending[stream] = b''
                        continue
                    if stream is child.stderr:
                        sys.stderr.buffer.write(chunk)
                        sys.stderr.buffer.flush()
                    lines = (pending[stream] + chunk).split(b'\n')
                    pending[stream] = lines.pop()
                    for line in lines:
                        if stream is child.stdout:
                            progress.line(line + b'\n')
                        else:
                            progress.unittest_line(line)
                    if stream is child.stderr:
                        progress.unittest_line(pending[stream])
            status = child.wait()
            return status if status >= 0 else 128 - status
    finally:
        if child is not None:
            child.terminate()
            try:
                child.wait(timeout=0.5)
            except subprocess.TimeoutExpired:
                pass
            reap_owned_children(child)
            child.stdout.close()
            child.stderr.close()
        libc.prctl(36, previous_subreaper.value, 0, 0, 0)
        for sig, handler in previous.items():
            signal.signal(sig, handler)


if __name__ == '__main__':
    os.environ['HIRSEL_TEST_REPORT_ACTIVE'] = '1'
    args = sys.argv[1:]
    raw = '--format=json' in args or '--list' in args
    raw = raw or any(pair == ['--format', 'json'] for pair in (args[i:i+2] for i in range(len(args))))
    sys.exit(supervise(args, raw_json=raw))
