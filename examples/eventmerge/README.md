# eventmerge

Merge two overlapping job exports before handing events to another agent. UUIDs
are canonicalized, identical events are counted once, and conflicting payloads
reject the merge with both original source locations. No partial event output is
emitted on failure.

From this directory, with a built `hirsel` on PATH:

```sh
hirsel check hirsel.json
hirsel export --tir hirsel.json -o /tmp/eventmerge.tir
hirsel run hirsel.json --grants grants.json
hirsel build hirsel.json --backend llvm -o /tmp/eventmerge
/tmp/eventmerge --hirsel-grants grants.json job-a.ndjson job-b.ndjson
```

`grants.json` maps only `fixtures/` to the read-only virtual directory `input`
and grants stdout. Relative host paths resolve beside the grants file. To try a
conflict, copy the grants file beside it and change its args to
`["job-a.ndjson", "conflict.ndjson"]`. For help, use `["--help"]`; no directory
grant is needed. Native invocation takes the same two filenames as positional
arguments. Cranelift uses `--backend cranelift`.

The success fixture combines an ingest job and an index job, recognizes a retried
export with reordered object fields and an uppercase URN ID, and emits three
unique events, one duplicate, and four total records. The conflict fixture says
the same ingest job failed: it exits 1, names `job-a.ndjson:1` and
`conflict.ndjson:1`, and includes the canonical ID.

Each nonempty record is exactly a JSON object with `id` (String) and `payload`
(any JSON value). Extra fields, missing fields, duplicate decoded keys, invalid
UTF-8 and blank lines are rejected. LF delimits records; CRLF works as JSON
trailing whitespace, and the final LF is optional. An empty export is valid.
Filenames are single components of 1–128 UTF-8 bytes, excluding `.`, `..`,
slash, backslash, colon and NUL.

IDs use shipped `uuid.parse` and `uuid.to_string`. Accepted spellings are exactly
32 hex digits, hyphenated 8-4-4-4-12 hex, that hyphenated form enclosed in actual
`{}` braces, or that form prefixed by case-insensitive `urn:uuid:`. Hex digits
are case-insensitive. Whitespace and other wrappers are rejected. All 128 bits
define identity; version and variant metadata neither filter nor order events.
No random IDs or clocks are used. An opaque application key holds the canonical
lowercase hyphenated spelling and supplies Map identity.

Payload equality is parsed JSON structural equality: decoded strings and bools
compare by value, null equals null, arrays compare recursively in order, and
objects compare recursively by decoded key regardless of insertion order.
Numbers use std's validated `Number(String)` token representation: `1`, `1.0`,
`1e0`, `0` and `-0` remain distinct spellings. There is no semantic numeric
normalization. Output retains the first event's payload, including its object
key order and number spelling; insignificant input whitespace is discarded.

Bounds are inclusive: **32768 bytes per file** (65536 total), **4096 bytes per
record excluding LF but including CR**, **64 records combined**, hence at most
64 unique events and 64 Map entries. Reads request at most 4096 bytes and probe
at most one byte beyond the file limit, rejecting before retaining excess input.
Record length and record count are checked before parsing/accumulating each
record. Std JSON limits nesting to 128 open containers. Both bounded files are
read before merging; file-read failures precede validation. Left file records
precede right file records, with one-based line numbers; line 0 denotes an I/O
or argument error. The first conflicting duplicate stops validation.

Stdout is one compact JSON report plus LF. Success has `class`, `events`,
`unique`, `duplicates`, and `records`; events appear in first-seen order.
Rejections have `class`, `error`, `source`, `line`, `first_source`, `first_line`,
and `id`. Non-conflict errors use an empty first source/ID and first line 0.
`InputError` retains typed JSON/UUID causes internally; the wire report exposes
stable application categories. Exit statuses are 0 for success/help, 1 for a
conflict, and 2 for invalid input, missing grants or I/O failure. Missing/broken
stdout can only be signalled by status 2. Application stderr is empty.

The focused `eventmerge_23` integration tests embed both source modules and all
fixtures, compare exact stdout/stderr/status against an independent Python
UUID/JSON oracle across interpreter, LLVM and Cranelift, and exercise checked
heap execution of the pure model and RFC-order UUID byte vectors. Fixtures are
hashed before/after; temporary input/grant cleanup is asserted. The tool creates
no processes or sockets and requires no write, process, network or clock grants.

Reference vectors adapt google/uuid `uuid_test.go:20–58,129–135` at
`2d3c2a9cc518326daf99a383f07c4d3c44317e4d`, with parsing usage from
`uuid.go:95–145`. [BSD-3-Clause license](fixtures/LICENSE) is retained. Deviations:
consume shipped Hirsel parsing/formatting; require actual braces (upstream's
38-byte branch does not validate its wrapper); test RFC-order binary identity
through `from_bytes`/`to_bytes` as well as upstream's text/case structure. The
checked-in vectors preserve expected canonical strings and all 16 bytes. Event
merge, bounds and equality cases are application additions.
