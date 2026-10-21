#!/usr/bin/env python3
"""Observe native startup inside perf/time/capped; reap only our process tree.

CLI timing reports arrive at exit. Finite programs may finish after their first
byte; tasks gets `quit` after announcing its listener; other servers’ native child is stopped, allowing the CLI to report phases.
Counters/RSS include that bounded shutdown (wall_s is the readiness timestamp).
A server killed before CLI exit cannot supply a phase report.
"""
import argparse
import json
import http.client
import os
from pathlib import Path
import selectors
import signal
import socket
import subprocess
import sys
import time


def observe(command, endpoint, grace=1.0):
    tcp = endpoint.split(':')[1:] if endpoint.startswith(('tcp:', 'http:')) else None
    if tcp:
        # Refuse a pre-existing listener; never report another program's readiness.
        with socket.socket() as probe:
            probe.bind((tcp[0], int(tcp[1])))
    start = time.monotonic()
    child = subprocess.Popen(command, stdout=subprocess.PIPE, stderr=None,
                             stdin=subprocess.PIPE, start_new_session=True)
    ready, reason, output = None, None, bytearray()
    stopped = False

    def stop(signum=signal.SIGTERM):
        nonlocal stopped
        stopped = True
        try:
            os.killpg(child.pid, signum)
        except ProcessLookupError:
            pass

    def interrupted(signum, _frame):
        stop(signum)
        try:
            child.wait(timeout=1)
        except subprocess.TimeoutExpired:
            stop(signal.SIGKILL)
            child.wait()
        raise SystemExit(128 + signum)

    def stop_program():
        # Native launch_live waits for its executable child. End that child so
        # the CLI can unwind and emit its real timing report before it exits.
        nonlocal stopped
        descendants = set()
        for task in Path(f'/proc/{child.pid}/task').glob('*/children'):
            try:
                descendants.update(int(pid) for pid in task.read_text().split())
            except OSError:
                pass
        if not descendants:
            stop()
        else:
            stopped = True
            for pid in descendants:
                try:
                    os.kill(pid, signal.SIGTERM)
                except ProcessLookupError:
                    pass

    previous = {s: signal.signal(s, interrupted) for s in (signal.SIGINT, signal.SIGTERM)}
    selector = selectors.DefaultSelector()
    selector.register(child.stdout, selectors.EVENT_READ)
    try:
        while child.poll() is None:
            for key, _ in selector.select(0.01):
                data = os.read(key.fd, 4096)
                if not data:
                    selector.unregister(key.fileobj)
                else:
                    output.extend(data)
                    if ready is None and (endpoint == 'stdout' or
                                          endpoint == 'listening' and b'listening on http://' in output):
                        ready, reason = time.monotonic() - start, endpoint
            if ready is None and tcp:
                with socket.socket() as probe:
                    probe.settimeout(0.01)
                    if probe.connect_ex((tcp[0], int(tcp[1]))) == 0:
                        ready, reason = time.monotonic() - start, endpoint
                        if endpoint.startswith('http:'):
                            # Same valid-request/drained-body contract as the
                            # existing bench/micro/http/load.py:14,29-38. Record
                            # listening at connect; avoid an artificial EOF before
                            # the request that the application actually accepts.
                            probe.settimeout(1)
                            connection = http.client.HTTPConnection(tcp[0], int(tcp[1]), timeout=1)
                            connection.sock = probe
                            try:
                                connection.request('GET', '/', headers={'Connection': 'close'})
                                response = connection.getresponse()
                                response.read()
                                if response.status != 200:
                                    raise ValueError(f'HTTP readiness returned {response.status}')
                            finally:
                                connection.close()
            if ready is not None:
                if endpoint == 'listening' and child.stdin and not child.stdin.closed:
                    try:
                        child.stdin.write(b'quit\n')
                        child.stdin.flush()
                        child.stdin.close()
                    except BrokenPipeError:
                        pass
                if time.monotonic() - start - ready > grace:
                    stop_program()
                    break
        try:
            code = child.wait(timeout=1)
        except subprocess.TimeoutExpired:
            stop(signal.SIGKILL)
            code = child.wait()
        # A short-lived process may exit before the selector reads its first byte.
        try:
            os.killpg(child.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        remaining = child.stdout.read()
        if remaining and ready is None and endpoint == 'stdout':
            ready, reason = time.monotonic() - start, 'stdout-at-exit'
        accepted = code == 0 or stopped
        report = {'status': ('ready' if accepted else 'ready-then-failed') if ready is not None else 'no-output' if code == 0 else 'failed',
                  'wall_s': ready, 'endpoint': reason, 'child_exit': code,
                  'shutdown_in_counters': True, 'forced_stop': stopped}
        print('compiler-startup: ' + json.dumps(report), file=sys.stderr, flush=True)
        return 0 if code == 0 or ready is not None and stopped else code if code > 0 else 1
    finally:
        if child.poll() is None:
            stop(signal.SIGKILL)
            child.wait()
        # Descendants can survive a parent's clean exit; the session is ours.
        stop(signal.SIGKILL)
        selector.close()
        child.stdout.close()
        if not child.stdin.closed:
            child.stdin.close()
        for sig, handler in previous.items():
            signal.signal(sig, handler)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--endpoint', default='stdout')
    parser.add_argument('command', nargs=argparse.REMAINDER)
    args = parser.parse_args()
    command = args.command[1:] if args.command[:1] == ['--'] else args.command
    if not command:
        parser.error('a command is required')
    return observe(command, args.endpoint)


if __name__ == '__main__':
    sys.exit(main())
