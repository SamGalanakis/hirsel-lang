#!/usr/bin/env python3
"""Send newline-delimited JSON requests on one bounded workqueue connection."""
import json
import socket
import struct
import sys

def exact(stream, count):
    output = b''
    while len(output) < count:
        chunk = stream.recv(count - len(output))
        if not chunk:
            raise EOFError('service closed before the full reply')
        output += chunk
    return output

with socket.socket(socket.AF_UNIX) as stream:
    stream.settimeout(2)
    stream.connect(sys.argv[1])
    for number in range(513):
        line = sys.stdin.buffer.readline(8193)
        if not line:
            break
        if number >= 512:
            raise ValueError('512 requests per connection')
        if len(line) > 8192:
            raise ValueError('8192 bytes per frame')
        request = json.loads(line)
        payload = json.dumps(request, separators=(',', ':')).encode()
        stream.sendall(struct.pack('>I', len(payload)) + payload)
        size = struct.unpack('>I', exact(stream, 4))[0]
        if size == 0 or size > 8192:
            raise ValueError('invalid reply size')
        print(exact(stream, size).decode(), flush=True)
