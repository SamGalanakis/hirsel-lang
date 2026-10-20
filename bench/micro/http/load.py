#!/usr/bin/env python3
"""Independent CPU7 open-loop client for the finite keep-alive servers."""
import argparse
import json
from pathlib import Path
import selectors
import socket
import time


def run(port, rate, stop=None):
    with socket.create_connection(('127.0.0.1', port), timeout=.1) as connection:
        connection.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
        connection.setblocking(False)
        request = b'GET / HTTP/1.1\r\nHost: localhost\r\n\r\n'
        pending = b''
        latencies = []
        sent = completed = 0
        start = time.monotonic()
        with selectors.DefaultSelector() as selector:
            selector.register(connection, selectors.EVENT_READ)
            while completed < 1024:
                if stop is not None and stop.exists():
                    return None
                now = time.monotonic()
                while sent < 1024 and now >= start + sent / rate:
                    # These tiny local requests fit the send buffer at this load.
                    assert connection.send(request) == len(request)
                    sent += 1
                delay = max(0, start + sent / rate - time.monotonic()) if sent < 1024 else .1
                for _, _ in selector.select(delay):
                    data = connection.recv(65536)
                    if not data:
                        raise ValueError('server closed before all responses')
                    pending += data
                    while b'\r\n\r\n' in pending:
                        head, body = pending.split(b'\r\n\r\n', 1)
                        if len(body) < 12:
                            break
                        assert b' 200 ' in head and body[:12] == b'Hello, world'
                        pending = body[12:]
                        latencies.append((time.monotonic() - (start + completed / rate)) * 1e6)
                        completed += 1
        return {'port': port, 'rate': rate, 'requests': completed,
                'elapsed': time.monotonic() - start, 'p50_us': sorted(latencies)[512],
                'p99_us': sorted(latencies)[1013], 'max_us': max(latencies)}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', required=True)
    parser.add_argument('--sessions', type=int, default=20)
    parser.add_argument('--rate', type=int, default=2048)
    parser.add_argument('--stop-file', type=Path, help='end promptly when the owner creates this file')
    args = parser.parse_args()
    rows = []
    deadline = time.monotonic() + 850
    while len(rows) < args.sessions and time.monotonic() < deadline:
        if args.stop_file is not None and args.stop_file.exists():
            return
        for port in (18475, 18476):
            try:
                row = run(port, args.rate, args.stop_file)
            except (ConnectionRefusedError, TimeoutError):
                continue
            if row is None:
                return
            rows.append(row)
            with open(args.output, 'w') as output:
                json.dump(rows, output, indent=2)
        time.sleep(.005)
    assert len(rows) == args.sessions, len(rows)


if __name__ == '__main__':
    main()
