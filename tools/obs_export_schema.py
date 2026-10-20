#!/usr/bin/env python3
"""Regenerate the collector test's JSON mapping from the pinned OTLP proto tree.

Requires protoc and Python protobuf only for regeneration, not for running tests.
Usage: uv run --with protobuf tools/obs_export_schema.py /tmp/ref-opentelemetry-proto
"""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import tempfile

PIN = '09769935deb8b2ebcf73ad1e2142b691d17bf70b'


def generate(root):
    from google.protobuf.descriptor_pb2 import FileDescriptorSet
    revision = subprocess.check_output(['git', '-C', str(root), 'rev-parse', 'HEAD'], text=True).strip()
    if revision != PIN:
        raise ValueError(f'OTLP checkout must be pinned to {PIN}')
    with tempfile.TemporaryDirectory() as temp:
        output = Path(temp) / 'descriptor.pb'
        subprocess.run(['protoc', '-I', str(root), '--include_imports', f'--descriptor_set_out={output}',
                        *[f'opentelemetry/proto/collector/{signal}/v1/{signal}_service.proto'
                          for signal in ('logs', 'trace', 'metrics')]], check=True)
        descriptor = FileDescriptorSet.FromString(output.read_bytes())
    messages, enums, sources = {}, {}, {}
    def enum(e, prefix):
        enums[prefix + '.' + e.name] = [v.number for v in e.value]
    def message(m, prefix):
        name = prefix + '.' + m.name
        fields = {}
        for f in m.field:
            field = {'type': f.type_name.lstrip('.') or f.Type.Name(f.type).removeprefix('TYPE_').lower()}
            if f.label == f.LABEL_REPEATED:
                field['repeated'] = True
            if f.HasField('oneof_index'):
                field['oneof'] = f.oneof_index
            fields[f.json_name] = field
        messages[name] = fields
        for e in m.enum_type:
            enum(e, name)
        for n in m.nested_type:
            message(n, name)
    for f in descriptor.file:
        sources[f.name] = hashlib.sha256((root / f.name).read_bytes()).hexdigest()
        for e in f.enum_type:
            enum(e, f.package)
        for m in f.message_type:
            message(m, f.package)
    return dict(pin=PIN, sources=sources, messages=messages, enums=enums)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('checkout', type=Path)
    parser.add_argument('-o', '--output', type=Path,
                        default=Path(__file__).parent / 'tests/fixtures/obs-export/proto-json-schema.json')
    args = parser.parse_args()
    args.output.write_text(json.dumps(generate(args.checkout), indent=2, sort_keys=True) + '\n')
