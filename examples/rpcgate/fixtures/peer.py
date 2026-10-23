#!/usr/bin/env python3
"""Small independent arithmetic/string peer; four workers, 64 connections."""
import concurrent.futures
import json
from pathlib import Path
import socket
import struct
import sys


def exact(stream, count):
    data = b''
    while len(data) < count:
        part = stream.recv(count - len(data))
        if not part:
            raise EOFError('short frame')
        data += part
    return data


def answer(stream):
    with stream:
        stream.settimeout(3)
        length, = struct.unpack('!I', exact(stream, 4))
        if not 0 < length <= 2048:
            return
        request = json.loads(exact(stream, length))
        method, args = request['method'], request['args']
        error = None
        if method == 'Arith.Add':
            result = args['a'] + args['b']
        elif method == 'Arith.Mul':
            result = args['a'] * args['b']
        elif method == 'Arith.Div':
            if args['b'] == 0:
                error = 'divide_by_zero'
                result = None
            else:
                result = abs(args['a']) // abs(args['b'])
                if (args['a'] < 0) != (args['b'] < 0):
                    result = -result
        elif method == 'echo':
            result = args
        elif method == 'normalize':
            result = args.strip().translate(str.maketrans(
                'ABCDEFGHIJKLMNOPQRSTUVWXYZ', 'abcdefghijklmnopqrstuvwxyz'))
        else:
            error, result = 'unknown_method', None
        if isinstance(result, int) and not -(2**63) <= result < 2**63:
            error = 'overflow'
        reply = dict(id=request['id'])
        reply.update(error=dict(code=error)) if error else reply.update(result=result)
        payload = json.dumps(reply, separators=(',', ':')).encode()
        if 0 < len(payload) <= 2048:
            stream.sendall(struct.pack('!I', len(payload)) + payload)


if __name__ == '__main__':
    path = Path(sys.argv[1])
    with socket.socket(socket.AF_UNIX) as listener:
        listener.bind(str(path))
        identity = path.stat()
        try:
            listener.listen(8)
            print('ready', flush=True)
            with concurrent.futures.ThreadPoolExecutor(max_workers=4) as workers:
                for _ in range(64):
                    stream, _ = listener.accept()
                    workers.submit(answer, stream)
        finally:
            current = path.stat() if path.exists() else None
            if current and (current.st_dev, current.st_ino) == (identity.st_dev, identity.st_ino):
                path.unlink()
