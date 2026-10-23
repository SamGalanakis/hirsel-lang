#!/usr/bin/env python3
"""One bounded request; leave send open until its correlated reply arrives."""
import json
import socket
import struct
import sys
from peer import exact

payload = sys.argv[2].encode()
assert 0 < len(payload) <= 2048
with socket.socket(socket.AF_UNIX) as stream:
    stream.settimeout(4)
    stream.connect(sys.argv[1])
    stream.sendall(struct.pack('!I', len(payload)) + payload)
    size, = struct.unpack('!I', exact(stream, 4))
    assert 0 < size <= 2048
    reply = json.loads(exact(stream, size))
    assert reply['id'] == json.loads(payload)['id']
    print(json.dumps(reply, separators=(',', ':')))
