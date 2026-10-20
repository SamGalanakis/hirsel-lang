# Conformance testee

`main.hir` is the testee for Google's protobuf conformance suite
(`conformance/conformance.proto`). It decodes each `ConformanceRequest` and
re-encodes the payload as binary or JSON. `messages.hir` is protoc@1's output for
`proto/test_messages_proto3.proto` and `proto/conformance.proto` (upstream copies).
Regenerate it with:

```
hirsel run ../protoc/hirsel.json --grants <grants mounting proto/ at ""> -- \
  messages.hir test_messages_proto3.proto conformance.proto
hirsel fmt messages.hir
```

Stdout takes only text (`// GAP:` in `main.hir`), so the testee answers each request
with one base64 line, and `adapter.py` turns the lines back into the runner's
length-prefixed frames. To run the suite:

```
hirsel build hirsel.json -o testee
conformance_test_runner --maximum_edition PROTO3 adapter.py
```

## Results

Recorded 2026-10-20: the testee built natively (LLVM), against the runner built
from protobuf main.

| Suite | Passed | Failed | Skipped |
|---|---|---|---|
| Binary and JSON | 1503 | 0 | 1314 |
| Text format | 0 | 0 | 445 |

There is no failure list. Every required and recommended proto3 binary and JSON case
passes. The skipped cases are out of scope:

- The 1314 in the binary and JSON suite are all `protobuf_test_messages.proto2`
  messages, answered `skipped`.
- The 445 in the text-format suite are text-format input (419 proto3, 18 proto2) and
  8 proto3 cases that ask for text-format output.
