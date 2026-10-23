# bininspect

Inspect a cached CBOR or MessagePack response as compact JSON. Select the format
explicitly; the tool decodes exactly one value and rejects trailing bytes. JSON
integers retain exact decimal lexemes, including `18446744073709551615`.

From the repository root, with `hirsel` on PATH:

```sh
hirsel check examples/bininspect/hirsel.json
hirsel run examples/bininspect/hirsel.json --grants examples/bininspect/grants.json -- cbor cache.cbor
hirsel run examples/bininspect/hirsel.json --grants examples/bininspect/grants.json -- msgpack cache.msgpack
hirsel run examples/bininspect/hirsel.json --grants examples/bininspect/grants.json -- cbor invalid.cbor
hirsel run examples/bininspect/hirsel.json --grants examples/bininspect/grants.json -- --help
```

The cache fixtures contain a request ID, HTTP status, cache flag and build record.
Both print exactly:

```json
{"request_id":18446744073709551615,"status":200,"cached":true,"items":[{"name":"build","ok":true}]}
```

The invalid cached body contains a CBOR byte string. It exits 2, prints no stdout,
and emits this report to stderr, preserving the codec's zero-based byte offset:

```json
{"error":"invalid_input","kind":"Unsupported","offset":6}
```

`--encode` parses JSON and writes raw binary stdout, without an added newline.
`--deterministic` is accepted only after `--encode` for CBOR; it sorts map keys by
preferred encoded-byte order and selects the shortest exact float width.

```sh
hirsel run examples/bininspect/hirsel.json --grants examples/bininspect/grants.json -- cbor cache.json --encode --deterministic > /tmp/cache.cbor
hirsel build examples/bininspect/hirsel.json --backend llvm -o /tmp/bininspect
/tmp/bininspect --hirsel-grants examples/bininspect/grants.json msgpack cache.msgpack
# The same build command accepts --backend cranelift.
```

The grants template resolves `fixtures` relative to its own directory. To inspect
other data, copy the template outside the repository and replace its `host` with
the absolute input directory. Arguments are relative to the logical `input`
directory. Grants cover only that directory (read access), stdout and stderr.
No file writes, processes, sockets, clocks or environment access are requested.
Shell redirection is performed by the caller, outside the program's authority.

Exit status is **0** for success/help and **2** for invalid input, usage, missing
application grants or I/O failure. If stdout/stderr is unavailable or fails, a
report may be impossible. Compiler/launcher refusals have their own CLI status.
Decode success is compact UTF-8 JSON plus one LF, with insertion-ordered object
keys and stable numeric normalization. Repeated runs on the same input produce
identical bytes. Output keys are not globally sorted; only deterministic CBOR
encoding promises independence from input object order.

Reports have `error`, `kind`, `offset` in that order. Wire kinds are `Truncated`,
`Syntax`, `Unsupported`, `Utf8`, `DuplicateKey`, `Nesting` and `Trailing` with
byte offsets; `InvalidNumber` has null offset. UTF-8 offsets identify the string
payload start; duplicate offsets identify the repeated key header. JSON parse
kinds use the `Json` prefix. Application policy/I/O kinds use lowercase names
and null offset, including `input_bytes_limit`, `collection_items_limit`,
`directory_grant`, `file_open`, `file_read`, `format` and `usage`.
Encoder nesting offsets, if reached, describe depth rather than source bytes.

Limits apply to both modes: **65,536 input bytes**, **64 elements/entries in
each array/map**, and the shipped libraries' **128 open containers**. One input
value is one record. Reads probe at most one byte beyond the input cap and
reject before appending that byte or calling a codec. The byte cap bounds the
initial DOM, including malformed/oversized collection declarations: every wire
node consumes at least one byte, so at most 65,536 wire nodes can be decoded;
std checks definite collection counts against remaining bytes before growth.
The 64-item policy is checked recursively on that bounded DOM before output or
encoding, rather than requiring a second parser. Strings share the input byte
bound. These are input/work bounds, not a fixed heap/RSS guarantee; escaped JSON
output and encoder buffers can be larger than the input.

Binary strings, CBOR tags/simple values, MessagePack extensions, non-text map
keys and nonfinite floats are unsupported, never coerced. Duplicate keys fail.
CBOR accepts indefinite containers and nonpreferred widths. Finite floats use
normalized binary64 decimal text (including `.0` and signed zero); integer
lexemes remain exact. Re-encoding preserves domain values, not original wire
widths, map order in deterministic mode, decimal spellings or arbitrary JSON
numeric precision. Integer/float encodings outside the selected format's range
return `InvalidNumber`.

Acceptance uses actual CLI `run` and `build --backend llvm/cranelift`, reachable
TIR export, and checked-H pure-model execution with zero live heap bytes.
Runtime fixtures run one at a time under `tools/capped` (60 seconds / 2 GiB).
The harness compares stdout, stderr and status with upstream vectors and a small
independent Python/`struct` encoder, checks input snapshots and a confined-path
sentinel, and asserts temporary cleanup. Test inputs and binaries stay private.

```sh
.buck2/bin/buck2 test -c hirsel.tests=focus //:hirsel-cli-test-cli-shard-1 -- --test-arg=bininspect_23
```

Prior art: ciborium `ciborium/src/de/mod.rs:1–35` at
`00279e48e75ecf9546aae36f6d004447c676065b` (Apache-2.0), CBOR test-vectors
`appendix_a.json:1–34` at `aba89b653e484bc8573c22f3ff35641d79dfd8c1`
(Simplified BSD, IETF Trust), and rmpv `src/decode/value.rs:1–40` plus
`rmpv-tests/tests/decode.rs:50–90` at
`cf880019f72e35a884659a9b29d9eb89b862670b` (MIT, Copyright (c) 2017 Evgeny
Safronov). See [third-party notices](../../third-party/NOTICES.md).
Usage follows header dispatch, bounded incremental input and JSON-domain failure
classification; codec implementations remain in std. Deviations are explicit
capabilities, a fixed input/collection policy, Unsupported instead of arbitrary
binary/tag/ext values, and typed JSON reports. Vector facts are retained in
`fixtures/vectors.json`; application/error controls are independently assembled
from the wire grammars. The Python encoder is only a bounded fixture oracle.

[FRICTION.md](FRICTION.md) records attempted forms and the final plumbing.
