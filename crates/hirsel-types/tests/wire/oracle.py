#!/usr/bin/env python3
"""Independent SER-2 v1 reader/writer. No Rust/native layout helpers are used.

The expected descriptor is supplied by the caller, never accepted as authority
from input. The oracle is an executable byte specification, not a source opt-in
checker. Its finite host budgets are operational refusal.
"""
import json
from pathlib import Path


class DecodeError(Exception):
    pass


class Reader:
    def __init__(self, data):
        self.data, self.pos = data, 0

    def take(self, size):
        if size > (1 << 64) - 1 - self.pos:
            raise DecodeError("Length")
        if size > len(self.data) - self.pos:
            raise DecodeError("Truncated")
        start = self.pos
        self.pos += size
        return self.data[start:self.pos]

    def uint(self, width):
        return int.from_bytes(self.take(width), "little", signed=False)

    def text(self):
        try:
            return self.take(self.uint(8)).decode("utf-8", errors="strict")
        except UnicodeDecodeError:
            raise DecodeError("Utf8") from None

    def done(self):
        if self.pos != len(self.data):
            raise DecodeError("Trailing")


def parse_descriptor(data):
    r = Reader(data)
    if r.uint(4) != 1:
        raise DecodeError("Schema")
    root, count = r.uint(4), r.uint(4)
    if count > 1000000:
        raise DecodeError("Limit")
    nodes = []
    for _ in range(count):
        tag = r.uint(1)
        if tag == 0:
            scalar = r.uint(1)
            if scalar > 14:
                raise DecodeError("Schema")
            nodes.append(("scalar", scalar))
        elif tag == 1:
            nodes.append(("fields", [r.uint(4) for _ in range(r.uint(4))]))
        elif tag == 2:
            names, children = [], []
            for _ in range(r.uint(4)):
                names.append(r.text())
                children.append(r.uint(4))
            if len(set(names)) != len(names):
                raise DecodeError("Schema")
            nodes.append(("fields", children))
        elif tag == 3:
            names, arms = [], []
            for _ in range(r.uint(4)):
                names.append(r.text())
                fields, labels = [], []
                for _ in range(r.uint(4)):
                    has_name = r.uint(1)
                    if has_name == 1:
                        labels.append(r.text())
                    elif has_name != 0:
                        raise DecodeError("Schema")
                    fields.append(r.uint(4))
                if len(set(labels)) != len(labels):
                    raise DecodeError("Schema")
                arms.append(fields)
            if len(set(names)) != len(names):
                raise DecodeError("Schema")
            nodes.append(("variant", arms))
        elif tag == 4:
            nodes.append(("wrap", r.uint(4)))
        elif tag == 5:
            kind, args = r.uint(1), []
            for _ in range(r.uint(4)):
                form = r.uint(1)
                if form not in (0, 1):
                    raise DecodeError("Schema")
                args.append((form, r.uint(4 if form == 0 else 8)))
            if kind in (0, 1) and len(args) == 1 and args[0][0] == 0:
                node = ("seq", args[0][1], None)
            elif kind in (2, 3) and len(args) == 2 and [a[0] for a in args] == [0, 1]:
                node = ("seq", args[0][1], args[1][1])
            elif kind == 4 and len(args) == 1 and args[0][0] == 0:
                node = ("wrap", args[0][1])
            elif kind == 5 and len(args) == 2 and all(a[0] == 0 for a in args):
                node = ("map", args[0][1], args[1][1])
            elif kind == 6 and len(args) == 1 and args[0][0] == 0:
                node = ("set", args[0][1])
            else:
                raise DecodeError("Schema")
            nodes.append(node)
        elif tag == 6:
            r.text()  # Exact nominal package/version/modules/name bind identity.
            r.text()
            for _ in range(r.uint(4)):
                r.text()
            r.text()
            nodes.append(("wrap", r.uint(4)))
        else:
            raise DecodeError("Schema")
    r.done()
    if root >= len(nodes):
        raise DecodeError("Schema")
    for node in nodes:
        if node[0] == "fields":
            edges = node[1]
        elif node[0] == "variant":
            edges = [t for arm in node[1] for t in arm]
        elif node[0] == "map":
            edges = node[1:]
        elif node[0] in ("wrap", "set", "seq"):
            edges = [node[1]]
        else:
            edges = []
        if any(t >= len(nodes) for t in edges):
            raise DecodeError("Schema")
    return root, nodes


def decode_payload(descriptor, data):
    root, nodes = parse_descriptor(descriptor)
    r, remaining = Reader(data), [1000000]

    def read(t, depth=0):
        remaining[0] -= 1
        if remaining[0] < 0 or depth > 256:
            raise DecodeError("Limit")
        node = nodes[t]
        kind = node[0]
        if kind == "scalar":
            p = node[1]
            if p == 0:
                b = r.uint(1)
                if b > 1:
                    raise DecodeError("Scalar")
                return ["bool", bool(b)]
            if 1 <= p <= 10:
                signed = p >= 6
                width = [1, 2, 4, 8, 16][(p - 1) % 5]
                return ["i" if signed else "u", str(int.from_bytes(r.take(width), "little", signed=signed))]
            if p in (11, 12):
                width, exponent, canonical = (4, 0x7f800000, 0x7fc00000) if p == 11 else (8, 0x7ff0000000000000, 0x7ff8000000000000)
                bits = r.uint(width)
                magnitude = bits & ((1 << (width * 8 - 1)) - 1)
                if magnitude > exponent and bits != canonical:
                    raise DecodeError("Scalar")
                return ["f32" if p == 11 else "f64", f"{bits:0{width*2}x}"]
            if p == 13:
                return ["string", r.text()]
            if p == 14:
                return ["bytes", r.take(r.uint(8)).hex()]
        if kind == "fields":
            return ["fields", [read(t, depth+1) for t in node[1]]]
        if kind == "variant":
            ordinal = r.uint(4)
            if ordinal >= len(node[1]):
                raise DecodeError("Tag")
            return ["variant", ordinal, [read(t, depth+1) for t in node[1][ordinal]]]
        if kind == "wrap":
            return ["wrap", read(node[1], depth+1)]
        n = node[2] if kind == "seq" and node[2] is not None else r.uint(8)
        if n > remaining[0] // (2 if kind == "map" else 1):
            raise DecodeError("Limit")
        result, keys, previous = [], set(), None
        for _ in range(n):
            start = r.pos
            item = read(node[1], depth+1)
            encoded = r.data[start:r.pos]
            if kind == "map":
                if encoded in keys:
                    raise DecodeError("Duplicate")
                keys.add(encoded)
                result.append([item, read(node[2], depth+1)])
            else:
                if kind == "set" and previous is not None:
                    if previous == encoded:
                        raise DecodeError("Duplicate")
                    if previous > encoded:
                        raise DecodeError("Order")
                previous = encoded
                result.append(item)
        return [kind, result]

    value = read(root)
    r.done()
    return value


def encode_payload(descriptor, value):
    root, nodes = parse_descriptor(descriptor)

    def uint(n, width):
        return n.to_bytes(width, "little", signed=False)

    def write(t, value):
        node = nodes[t]
        kind = node[0]
        if kind == "scalar":
            p = node[1]
            if p == 0:
                return bytes([int(value[1])])
            if 1 <= p <= 10:
                return int(value[1]).to_bytes([1, 2, 4, 8, 16][(p-1) % 5], "little", signed=p >= 6)
            if p in (11, 12):
                return uint(int(value[1], 16), 4 if p == 11 else 8)
            content = value[1].encode("utf-8") if p == 13 else bytes.fromhex(value[1])
            return uint(len(content), 8) + content
        if kind == "fields":
            return b"".join(write(t, v) for t, v in zip(node[1], value[1], strict=True))
        if kind == "variant":
            return uint(value[1], 4) + b"".join(write(t, v) for t, v in zip(node[1][value[1]], value[2], strict=True))
        if kind == "wrap":
            return write(node[1], value[1])
        content = value[1]
        prefix = b"" if kind == "seq" and node[2] is not None else uint(len(content), 8)
        if kind == "map":
            return prefix + b"".join(write(node[1], k) + write(node[2], v) for k, v in content)
        parts = [write(node[1], v) for v in content]
        if kind == "set":
            parts.sort()
        return prefix + b"".join(parts)

    return write(root, value)


def envelope(descriptor, payload):
    return b"HSC1" + (1).to_bytes(4, "little") + len(descriptor).to_bytes(8, "little") + descriptor + len(payload).to_bytes(8, "little") + payload


def decode_envelope(data, expected):
    r = Reader(data)
    if r.take(4) != b"HSC1" or r.uint(4) != 1:
        raise DecodeError("Version")
    descriptor = r.take(r.uint(8))
    if descriptor != expected:
        raise DecodeError("SchemaMismatch")
    payload = r.take(r.uint(8))
    r.done()
    return decode_payload(descriptor, payload)


def main():
    directory = Path(__file__).parent
    corpus = {}
    for line in (directory / "golden.tsv").read_text().splitlines():
        if not line or line.startswith("#"):
            continue
        name, schema, payload, value = line.split("\t")
        descriptor, raw = bytes.fromhex(schema), bytes.fromhex(payload)
        expected = json.loads(value)
        assert decode_payload(descriptor, raw) == expected, name
        assert encode_payload(descriptor, expected) == raw, name
        blob = envelope(descriptor, raw)
        assert decode_envelope(blob, descriptor) == expected, name
        for position in range(len(blob)):
            try:
                decode_envelope(blob[:position], descriptor)
            except DecodeError:
                pass
            else:
                raise AssertionError((name, "accepted truncated envelope", position))
        corpus[name] = descriptor
    malformed = 0
    for line in (directory / "malformed.tsv").read_text().splitlines():
        if not line or line.startswith("#"):
            continue
        name, case, payload, error = line.split("\t")
        try:
            decode_payload(corpus[case], bytes.fromhex(payload))
        except DecodeError as failure:
            assert str(failure) == error, (name, str(failure), error)
        else:
            raise AssertionError((name, "accepted malformed input"))
        malformed += 1
    print(f"Independent SER-2 oracle: {len(corpus)} canonical vectors, {malformed} malformed vectors")


if __name__ == "__main__":
    main()
