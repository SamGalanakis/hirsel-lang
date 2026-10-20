#!/usr/bin/env python3
"""Run the 14 grpc-go non-auth interop cases in both directions.

Build main.hir as a native executable first, then:
  python3 check.py --hirsel /absolute/interop --go-client /absolute/client --go-server /absolute/server

The Go executables are interop/{client,server} at grpc-go 679b224.
Each case has a 30-second deadline; servers are always reaped. Logs and the
exact grants are retained in --output (default: a new temporary directory).
"""
import argparse
from contextlib import contextmanager
import json
from pathlib import Path
import socket
import subprocess
import tempfile
import time

CASES = (
    'empty_unary', 'large_unary', 'client_streaming', 'server_streaming',
    'ping_pong', 'cancel_after_begin', 'timeout_on_sleeping_server',
    'status_code_and_message', 'custom_metadata', 'empty_stream',
    'cancel_after_first_response', 'special_status_message',
    'unimplemented_method', 'unimplemented_service',
)


def free_port():
    with socket.socket() as listener:
        listener.bind(('127.0.0.1', 0))
        return listener.getsockname()[1]


@contextmanager
def server(command, log, port):
    with log.open('w') as output:
        process = subprocess.Popen(command, stdout=output, stderr=subprocess.STDOUT)
        try:
            # TCP startup probe is deliberately separate from the RPC assertions.
            deadline = time.monotonic() + 10
            while True:
                if process.poll() is not None:
                    raise RuntimeError(f'server exited: {log}')
                try:
                    with socket.create_connection(('127.0.0.1', port), timeout=.1):
                        break
                except OSError:
                    if time.monotonic() >= deadline:
                        raise RuntimeError(f'server did not listen: {log}') from None
                    # Wait for this process to exit, or a short startup interval.
                    try:
                        process.wait(timeout=.05)
                    except subprocess.TimeoutExpired:
                        pass
            yield
        finally:
            process.terminate()
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait()


def run_cases(command, output, direction):
    failures = []
    for case in CASES:
        log = output / f'{direction}-{case}.log'
        with log.open('w') as stream:
            try:
                result = subprocess.run(command(case), stdout=stream,
                                        stderr=subprocess.STDOUT, timeout=30)
                passed = result.returncode == 0
            except subprocess.TimeoutExpired:
                passed = False
        print(f'{direction} {case}: {"pass" if passed else "FAIL"}', flush=True)
        if not passed:
            failures.append(str(log))
    return failures


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ('hirsel', 'go-client', 'go-server'):
        parser.add_argument('--' + name, required=True, type=Path)
    parser.add_argument('--output', type=Path)
    args = parser.parse_args()
    output = args.output or Path(tempfile.mkdtemp(prefix='grpc-interop-'))
    output.mkdir(parents=True, exist_ok=True)
    port = free_port()
    grants = output / 'grants.json'
    grants.write_text(json.dumps({'io': {
        'stdout': 'stdout.bytes@1', 'tasks': 'tasks.cooperative@1',
        'monotonic_clock': 'clock.monotonic.ns@1', 'dirs': [],
        'net': {'provider': 'net.stream@1', 'rules': [
            {'address': '127.0.0.1', 'service': str(port), 'operation': op}
            for op in ('listen', 'connect')
        ]},
    }}, indent=2) + '\n')
    hirsel = [str(args.hirsel.resolve()), '--hirsel-grants', str(grants.resolve())]
    with server([str(args.go_server.resolve()), f'--port={port}'],
                output / 'go-server.log', port):
        failures = run_cases(lambda case: [*hirsel, 'client',
            '--server_host=127.0.0.1', f'--server_port={port}', f'--test_case={case}'],
            output, 'hirsel-client')
    with server([*hirsel, 'server', f'--port={port}'],
                output / 'hirsel-server.log', port):
        failures += run_cases(lambda case: [str(args.go_client.resolve()),
            '--server_host=127.0.0.1', f'--server_port={port}', f'--test_case={case}',
            '--use_tls=false'], output, 'go-client')
    print(f'{2 * len(CASES) - len(failures)}/{2 * len(CASES)} passed; logs: {output}')
    if failures:
        raise SystemExit('failed: ' + ', '.join(failures))


if __name__ == '__main__':
    main()
