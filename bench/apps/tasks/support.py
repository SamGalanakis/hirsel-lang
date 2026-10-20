"""Process/config helpers shared by the equivalence driver and benchmark."""
import json
import os
from pathlib import Path
import selectors
import socket
import subprocess
import time

ROOT = Path(__file__).resolve().parents[3]
APP = ROOT / 'bench/apps/tasks'
CAPPED = ROOT / 'tools/capped'
RUST = APP / 'rust/target/release/tasks-rust'
TOOLCHAIN = ROOT / 'buck-out/v2/art/root/967aa399796c0942/__hirsel-toolchain__/hirsel-toolchain/hirsel'


def line(process, timeout=30):
    with selectors.DefaultSelector() as selector:
        selector.register(process.stdout, selectors.EVENT_READ)
        if not selector.select(timeout):
            raise TimeoutError(f'no readiness line from pid {process.pid}')
        value = process.stdout.readline().decode().strip()
        if not value:
            raise RuntimeError(f'process exited before readiness: {process.args}')
        return value


def free_port():
    with socket.socket() as sock:
        sock.bind(('127.0.0.1', 0))
        return sock.getsockname()[1]


def grants(directory, port):
    value = json.loads((ROOT / 'examples/tasks/grants.json').read_text())
    value['io'].pop('wall_clock')
    value['io']['dirs'][0]['host'] = str(directory.resolve())
    value['io']['dirs'][1]['host'] = str(ROOT / 'examples/tasks/static')
    value['io']['net']['rules'][0]['service'] = str(port)
    return value


class Server:
    def __init__(self, kind, directory, port, hirsel_binary, log, timeout=300, perf=None, record=None):
        environment = dict(os.environ, TASKS_DATA=str(directory),
                           TASKS_STATIC=str(ROOT / 'examples/tasks/static'),
                           TASKS_WALL_CLOCK='off', TASKS_LOG_LEVEL='error',
                           TASKS_RUNTIME='multi_thread' if kind == 'rust-one-worker' else 'current_thread',
                           HIRSEL_JOBS='1')
        if kind == 'hirsel':
            command = [str(hirsel_binary), '--hirsel-grants', json.dumps(grants(directory, port)), '--port', str(port), '--log-level', 'error']
        else:
            command = [str(RUST), '--port', str(port)]
        self.marker = directory.parent / (directory.name + '-launch.json')
        command = ['taskset', '-c', '31', str(APP / 'rust/target/release/launch'), str(self.marker), *command]
        if record:
            command = ['perf', 'record', '-e', 'instructions', '-c', '10000', '-g',
                       '-o', str(record), *command]
        if perf:
            environment['TASKS_PERF_WRAPPER'] = '1'
            command = ['perf', 'stat', '-x', ';', '-o', str(perf), '-e',
                       'task-clock,cycles,instructions,cycles:u,instructions:u', *command]
        self.log = open(log, 'wb')
        self.started = time.monotonic()
        self.process = subprocess.Popen([str(CAPPED), *(['--long'] if timeout > 900 else []),
                                         '--timeout', str(timeout), *command],
                                        env=environment, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                        stderr=self.log, bufsize=0)
        self.port = port
        try:
            value = line(self.process, 60)
            assert value == f'listening on http://127.0.0.1:{port}/', value
        except BaseException:
            self.stop(check=False)
            raise RuntimeError(f'server startup: {Path(log).read_text()}')

    def stop(self, check=True):
        if self.process.poll() is None:
            self.process.stdin.close()
            try:
                self.process.wait(30)
            except subprocess.TimeoutExpired:
                self.process.terminate()
                self.process.wait(15)
                raise
        self.log.close()
        if check and self.process.returncode:
            raise RuntimeError(f'server failed: exit {self.process.returncode}')

