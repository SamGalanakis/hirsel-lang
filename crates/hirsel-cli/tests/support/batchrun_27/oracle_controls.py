"""Permanent boundary controls, separate from the app's deadline acceptance.

Readiness follows Go exec_test.go:922-952 at ebe2feefc400f813aefdc8a036e66d2619da6649:
observe the handshake, cancel, then wait. No Go code or long wall waits copied.
The exec-stop control explicitly perturbs scheduling at successful exec.
"""
import argparse
import ctypes
import json
import os
from pathlib import Path
import select
import signal
import subprocess
import sys
import time

from child_accounting import LIBC, account, assert_reaped, trace


def pre_marker(root, fixture):
    # The parent owns and waits its subprocess; the observer records both that
    # wait and the kernel exit. Hold the unchanged fixture at successful exec
    # until the original 300 ms deadline, before loader/Python user code.
    output = root/'children.json'
    argv = [sys.executable, '-c',
            'import subprocess,sys; p=subprocess.Popen(sys.argv[1:],cwd="/",env={}); p.wait()',
            '/usr/bin/python3', str(fixture), 'hang', str(root), '0']
    assert trace(argv, output, hold_exec_until=time.monotonic() + .300) == 0
    markers = {p.stem: int(p.read_text()) for p in root.glob('*.started')}
    assert not markers
    children = account(json.loads(output.read_text()), markers)
    assert len(children) == 1 and children[0]['kernel_exit_status'] == signal.SIGKILL, children
    assert children[0]['state'] == 'killed before the marker', children
    return children


def ready_cancel(root, fixture):
    fd = LIBC.inotify_init1(os.O_CLOEXEC | os.O_NONBLOCK)
    assert fd >= 0
    child = None
    try:
        assert LIBC.inotify_add_watch(fd, os.fsencode(root), 0x8) >= 0  # CLOSE_WRITE
        child = subprocess.Popen(['/usr/bin/python3', str(fixture), 'hang', str(root), '0'],
                                 cwd='/', env={}, stdin=subprocess.DEVNULL,
                                 stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        until = time.monotonic() + 10
        while True:
            remaining = until - time.monotonic()
            assert remaining > 0 and select.select([fd], [], [], remaining)[0], 'no readiness handshake'
            os.read(fd, 65536)
            marker = root/'0.started'
            if marker.exists() and marker.read_text() == str(child.pid):
                break
        observed = time.monotonic_ns()
        child.kill()
        canceled = time.monotonic_ns()
        code = child.wait(timeout=10)
        assert code == -signal.SIGKILL and not Path('/proc', str(child.pid)).exists()
        return dict(state='ready then cancelled', pid=child.pid, observed_ns=observed,
                    canceled_ns=canceled, wait_returncode=code, reaped=True)
    finally:
        if child is not None:
            if child.poll() is None:
                child.kill()
            child.wait()
        os.close(fd)


def exec_failure(root, fixture):
    output = root/'children.json'
    argv = [sys.executable, '-c',
            'import subprocess,sys\ntry: subprocess.Popen([sys.argv[1]])\nexcept FileNotFoundError as e: assert e.errno == 2\nelse: raise AssertionError("unexpected exec success")',
            str(root/'absent-executable')]
    assert trace(argv, output) == 0
    children = account(json.loads(output.read_text()), {})
    assert len(children) == 1 and children[0]['state'] == 'exec failed', children
    assert children[0]['exec_error_errno'] == 2, children
    assert os.WIFEXITED(children[0]['kernel_exit_status']), children
    return children


def unreaped(root, fixture):
    pid = os.fork()
    if pid == 0:
        os._exit(0)
    try:
        info = os.waitid(os.P_PID, pid, os.WEXITED | os.WNOWAIT)
        assert info.si_pid == pid
        assert Path('/proc', str(pid), 'stat').read_text().split(') ', 1)[1].startswith('Z ')
        # A marker-free zombie and a gone PID without parent wait evidence
        # must both be rejected by the exact same oracle used by acceptance.
        try:
            account(dict(children=[dict(pid=pid, kernel_exit_status=0)]), {})
        except AssertionError as error:
            assert error.args[0][0] == 'unreaped', error
        else:
            raise AssertionError('marker-free zombie accepted')
    finally:
        _, status = os.waitpid(pid, 0)
    assert status == 0 and not Path('/proc', str(pid)).exists()
    for row in [dict(pid=pid, kernel_exit_status=0, tracer_wait_status=0),
                dict(pid=pid, kernel_exit_status=0, tracer_wait_status=0, parent_wait_status=9)]:
        try:
            assert_reaped(row)
        except AssertionError as error:
            assert error.args[0][0] == 'missing/exact wait proof', error
        else:
            raise AssertionError('missing or mismatched wait proof accepted')
    return dict(pid=pid, state='unreaped detected then reaped', wait_status=status)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('control', choices=['pre_marker','ready_cancel','exec_failure','unreaped'])
    parser.add_argument('root', type=Path)
    parser.add_argument('fixture', type=Path)
    args = parser.parse_args()
    result = globals()[args.control](args.root, args.fixture)
    print(json.dumps(dict(id=args.control, passed=True, result=result)))
