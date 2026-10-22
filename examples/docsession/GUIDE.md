# A bounded JSON document session

Run from the repository root with a matching Hirsel toolchain:

```sh
printf '%s\n' '{"io":{"stdin":"stdin.bytes@1","stdout":"stdout.bytes@1","dirs":[]},"args":[]}' > /tmp/docsession-grants.json
hirsel run examples/docsession/hirsel.json --grants /tmp/docsession-grants.json
```

Send `SHOW` and press Enter: the process responds immediately with
`{"ok":"value","value":null}` followed by LF. There is no prompt.
Each command is case sensitive. No leading/trailing whitespace is removed.
`SET `, `PATCH ` and `GET ` use exactly one separating ASCII space; everything
following that space is the operand. `GET` alone also selects the root.
JSON operands may contain JSON whitespace. Empty/unknown commands and extra
operands on other commands return `{"error":"UnknownCommand"}`.

Try this executable transcript (the input/output files are independent goldens):

```sh
hirsel run examples/docsession/hirsel.json --grants /tmp/docsession-grants.json \
  < examples/docsession/fixtures/transactions.in > /tmp/docsession.out
cmp examples/docsession/fixtures/transactions.out /tmp/docsession.out
hirsel test examples/docsession/hirsel.json
hirsel test examples/docsession/hirsel.json --native --backend llvm
hirsel test examples/docsession/hirsel.json --native --backend cranelift
```

| Command | Compact response (plus LF) |
| --- | --- |
| `SET <json>` | `{"ok":"set"}` |
| `PATCH <json>` | `{"ok":"patch"}` |
| `GET <pointer>` or `GET` | `{"ok":"value","value":<selected JSON>}` |
| `SHOW` | `{"ok":"value","value":<working JSON>}` |
| `BEGIN` | `{"ok":"begin"}` |
| `COMMIT` | `{"ok":"commit"}` |
| `ROLLBACK` | `{"ok":"rollback"}` |
| `HELP` | `{"ok":"help","commands":["SET <json>","PATCH <json>","GET <pointer>","SHOW","BEGIN","COMMIT","ROLLBACK","HELP","QUIT"]}` |
| `QUIT` | `{"ok":"quit"}` |

There is one working document, initially JSON null, and at most one snapshot.
BEGIN snapshots the working document. Updates and GET/SHOW inside a transaction
see its working value. COMMIT discards the snapshot; ROLLBACK restores it.
EOF and QUIT discard an open transaction silently (no rollback response).
QUIT responds once and ignores any remaining unread input.

PATCH follows RFC7396: an object merges members, null deletes a member, arrays
and other values replace. A root null patch sets JSON null. Updates are built
separately; errors never publish a partial document or alter the snapshot.
Numbers retain their lexical spelling. Serialization uses map insertion order;
merge-patch removes and reinserts replaced object members at the end.

Recoverable, typed command errors have exactly `{"error":"<arm>"}`:
`InvalidJson`, `DepthLimit`, `DocumentLimit`, `InvalidPointer`, `MissingPath`,
`TransactionOpen`, `NoTransaction`, `UnknownCommand`. Missing paths differ from
present JSON null. Pointers use RFC6901 string syntax, not URI fragments:
empty selects the root; `~0` means `~`, `~1` means `/`; malformed escapes are
InvalidPointer. Array indexes are `0` or a nonzero decimal without leading
zeros; `-`, overflowing indexes and absent children are MissingPath.

Limits: 8192 UTF-8 bytes per command, excluding LF or CRLF; 1024 commands per
session (including errors, HELP and QUIT); 65536 bytes in compact serialized
accepted documents; 32 open array/object containers. A scalar has depth zero.
The quote/escape-aware bracket preflight runs before recursive JSON parsing;
the JSON parser still decides syntax, duplicate keys and number validity.
Every candidate is serialized and checked again before publication, including
merged documents whose combined size/depth can exceed either input.

LF, CRLF and final unterminated commands work with fragmented reads. A trailing
CR is removed only when followed by LF. Framing/UTF-8/read/write errors and the
1025th command terminate with exit 1 and no response for that command; previous
response lines remain valid. EOF and QUIT exit 0. No filesystem, network,
environment or task capability is requested; only Stdin and Stdout are used.

Record and replay with original stdin/grants unavailable:

```sh
hirsel build examples/docsession/hirsel.json --backend llvm -o /tmp/docsession-llvm
hirsel build examples/docsession/hirsel.json --backend cranelift -o /tmp/docsession-cranelift
hirsel run examples/docsession/hirsel.json --grants /tmp/docsession-grants.json \
  --record /tmp/docsession.tape < examples/docsession/fixtures/transactions.in
mv /tmp/docsession-grants.json /tmp/docsession-grants.retired.json
hirsel run examples/docsession/hirsel.json --replay /tmp/docsession.tape < /dev/null
/tmp/docsession-llvm --hirsel-replay /tmp/docsession.tape < /dev/null
/tmp/docsession-cranelift --hirsel-replay /tmp/docsession.tape < /dev/null
```

The standalone correctness test also records successful and recoverable-error
sessions in every engine and checks all nine cross-engine replays per session.
It sends commands incrementally and requires each response before sending the
next command, preventing a whole-stdin implementation from passing.
EOF rollback has no externally visible persistent state: the process owns no
disk state. Explicit ROLLBACK tests verify restoration bytes.

Prior art: Go `bufio/scan.go:351–368`, `scan_test.go:175–213` at
`ebe2feefc400f813aefdc8a036e66d2619da6649` supply line framing and fragmented
boundary vectors; adapted to shipped bounded `bufio.stdin_read_line`, which
retains delimiters. Unlike Go's final `dropCR`, final bare CR stays command data.
Serde JSON `src/value/mod.rs:779–800`, `tests/test.rs:1830–1866` at
`20572a6d066958542d79cdd6cf1a7cd9f0c8e1f8` supply read-only RFC6901 lookup
structure and independent pointer vectors. Shipped `json_pointer` uses bounded
recursive borrowed descent for Hirsel ownership, validates escapes strictly,
and distinguishes invalid syntax from missing paths. This example calls that
API without introducing another traversal implementation. Shipped
`json_merge_patch.apply` supplies the RFC7396 implementation and its existing
json-patch MIT provenance. Reference licenses accompany the correctness helper.
