import base64
import gzip
import hashlib
import io
import pathlib
import random
import subprocess
import sys
import tarfile
import tempfile

for script in map(pathlib.Path, sys.argv[1:]):
    with tempfile.TemporaryDirectory() as scratch:
        root = pathlib.Path(scratch)
        rng = random.Random(48)
        expected = {f'{n}.json': rng.randbytes(800_000) for n in range(6)}
        for name, data in expected.items():
            (root / name).write_bytes(data)
        result = subprocess.run([sys.executable, str(script.resolve()), str(root)], capture_output=True)
        assert result.returncode == 0, result.stderr.decode()
        found, chunks = {}, 0
        lines = iter(result.stdout.decode().splitlines())
        for line in lines:
            marker, digest, size, encoded_size = line.split()
            assert marker.endswith('_ARCHIVE_BEGIN')
            encoded = ''
            for part in lines:
                if '_ARCHIVE_END ' in part:
                    assert part.split()[1] == digest
                    break
                encoded += part
            assert len(encoded) == int(encoded_size) <= 4 * 1024 * 1024
            body = base64.b64decode(encoded, validate=True)
            assert len(body) == int(size)
            assert hashlib.sha256(body).hexdigest() == digest
            with tarfile.open(fileobj=io.BytesIO(gzip.decompress(body))) as archive:
                for member in archive:
                    assert member.name not in found
                    found[member.name] = archive.extractfile(member).read()
            chunks += 1
        assert chunks > 1 and found == expected
        # One indivisible file must still obey the original bound.
        for path in root.iterdir():
            path.unlink()
        (root / 'large.json').write_bytes(rng.randbytes(3_200_000))
        result = subprocess.run([sys.executable, str(script.resolve()), str(root)], capture_output=True)
        assert result.returncode != 0 and b'exceeds 4MiB' in result.stderr
        print(f'{script.name}: bounded chunks retain all six payloads; indivisible overflow refused')
