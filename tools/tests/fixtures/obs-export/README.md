# OTLP fixture collector

Protocol pin: opentelemetry-proto `09769935deb8b2ebcf73ad1e2142b691d17bf70b`
(Apache-2.0, LICENSE here). `logs.json`, `trace.json`, and `metrics.json` are
unmodified upstream `examples/` vectors. The exporter ports their log body, trace
IDs/parent, server kind, histogram bounds/counts, units and timestamp conventions.
The fixture adds four Hirsel severities, a link, signed integer limits and explicit
Hirsel evidence/projection/clock/export provenance. No SDK or ambient context is
involved.

`view.json` is the obs-view ordinary fixture with original monotonic samples
100/200 added; the samples and clock correlation are synthetic *test evidence*,
not latency measurements of that program. `projection.json` supplies explicit
analysis records for testing all three signals. `correlation.json` supplies the
original Unix/monotonic correspondence used by this fixture.

`captured-{logs,traces,metrics}.json` are the HTTP request bodies received by the
fixture collector for export identity `fixture-export`. The test byte-compares
incoming payloads with them, then validates every field recursively against the
pinned proto descriptors' JSON mapping. Enum values are integers, 64-bit integers
are decimal strings, and trace/span IDs use OTLP hex, rather than ProtoJSON base64.
The test also checks empty and warning-only partial responses, rejection without
retry, a disconnect after acceptance, HTTP retry codes, and explicit re-export.

`proto-json-schema.json` is generated from `protoc --include_imports` descriptors
for the three Export services, including imported messages, field types, oneof
membership and enum numbers. It also records each source proto's SHA-256. Normal
tests use only Python's standard library. To reproduce the descriptor projection:

```sh
uv run --with protobuf tools/obs_export_schema.py /tmp/ref-opentelemetry-proto
python3 tools/tests/obs_export.py
```

Regeneration refuses a checkout at another revision. No request field schema is
hand-maintained in the collector.
