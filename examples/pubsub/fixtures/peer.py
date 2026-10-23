"""Run the real interpreter server and two independent bounded Unix peers."""
import json
from pathlib import Path
import socket
import struct
import subprocess
import sys
import tempfile

manifest = Path(__file__).resolve().parents[1] / 'hirsel.json'
with tempfile.TemporaryDirectory(prefix='pubsub-') as private:
    socket_path = str(Path(private) / 'relay.sock')
    grants = {'io': {'dirs': [], 'stdout': 'stdout.bytes@1',
        'tasks': 'tasks.cooperative@1', 'monotonic_clock': 'clock.monotonic.ns@1',
        'net': {'provider': 'net.stream@1', 'rules': [
            {'address': socket_path, 'service': '0', 'operation': 'unix_listen'}]}}}
    server = subprocess.Popen([sys.argv[1] if len(sys.argv) > 1 else 'hirsel',
        'run', str(manifest), '--grants', json.dumps(grants), '--',
        'serve', socket_path, '1', '5000', '4'], stdout=subprocess.PIPE)
    try:
        assert json.loads(server.stdout.readline()) == {'listening': True}
        def send(peer, message):
            data = json.dumps(message, separators=(',', ':')).encode()
            peer.sendall(struct.pack('>I', len(data)) + data)
        def receive(peer):
            def exact(count):
                data = b''
                while len(data) < count:
                    chunk = peer.recv(count - len(data))
                    if not chunk: raise EOFError('relay closed')
                    data += chunk
                return data
            size = struct.unpack('>I', exact(4))[0]
            if not 0 < size <= 2048: raise ValueError('frame bound')
            return json.loads(exact(size))
        with socket.socket(socket.AF_UNIX) as subscriber, socket.socket(socket.AF_UNIX) as publisher:
            subscriber.settimeout(2)
            publisher.settimeout(2)
            subscriber.connect(socket_path)
            publisher.connect(socket_path)
            send(subscriber, {'op': 'subscribe', 'topic': 'news'})
            assert receive(subscriber) == {'op': 'subscribed', 'topic': 'news'}
            send(publisher, {'op': 'publish', 'topic': 'news', 'data': 'hello'})
            print(receive(publisher))
            print(receive(subscriber))
        suffix, _ = server.communicate(timeout=10)
        print(suffix.decode(), end='')
        assert server.returncode == 0
    finally:
        if server.poll() is None:
            server.kill()
            server.wait()
