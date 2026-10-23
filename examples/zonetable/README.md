# zonetable

Convert a list of proposed local meeting times using explicit TZif files. This is
useful when an agent schedules a handoff across regions: a repeated 01:30 needs
an explicit choice, and a skipped 02:30 cannot be scheduled as written. No clock,
network, timezone database discovery, host local zone or locale inference is used.

Build the CLI using [Buck2 setup](../../tools/buck2/README.md), then run from the
repository root:

```sh
hirsel check examples/zonetable/hirsel.json
hirsel run examples/zonetable/hirsel.json --grants examples/zonetable/grants.json
hirsel run examples/zonetable/hirsel.json --grants examples/zonetable/grants.json -- invalid.json
hirsel run examples/zonetable/hirsel.json --grants examples/zonetable/grants.json -- --help
hirsel run examples/zonetable/hirsel.json --grants examples/zonetable/grants.json -- success.json --format '%Y-%m-%d %H:%M %z'
```

The supplied success list schedules a July New York handoff in UTC+05 and selects
the later November 01:30 occurrence in UTC. Its first result is
`2024-07-15T21:30:00Z` / `2024-07-16T02:30:00.0+0500`; its second is
`2024-11-03T06:30:00Z` / `2024-11-03T06:30:00.0+0000` with `resolution:"fold"`.

The input is a JSON array of objects with required string fields `local`,
`source`, `destination`, and optional string `policy`:

```json
[{"local":"2024-11-03T01:30:00","source":"new-york.tzif","destination":"utc.tzif","policy":"Later"}]
```

`local` follows `date_time.parse`: an ISO date, `T` or `t`, and `HH:MM` or
`HH:MM:SS` with optional one-to-nine-digit fraction; no offset. Calendar fields
must be valid. Unknown/missing fields, duplicate JSON keys, wrong kinds, trailing
data, invalid UTF-8 and unknown policies are rejected. Duplicate rows are allowed.
An empty array succeeds. JSON validation completes before zone reads; conversions
stop at the first failure in row order. No partial table is published.

Policies are case sensitive: **Reject** is the default and rejects gaps and folds;
**Earlier** selects the earlier instant; **Later** selects the later instant.
For New York's spring gap, 2024-03-10 02:30 Earlier selects 06:30Z, shown in New
York as 01:30; Later selects 07:30Z, shown as 03:30. Neither instant has the input
civil time. During the autumn fold, Earlier selects 05:30Z and Later 06:30Z, both
shown as 01:30. These are the shipped Hirsel/Jiff semantics.

Success is compact JSON plus LF, in input order. Each row includes the original
`local`, `source`, `destination`, effective `policy`, `resolution` (`single`, `gap`,
`fold`), selected UTC `instant`, selected source civil `source_time`, formatted
`destination_time`, and numeric `offset_seconds` east of UTC. The separate source
time exposes a gap shift. UTC instants must fit years 0000–9999 because
`time.to_string` produces RFC 3339; other civil years can parse but may fail with
`range`. TZif second-resolution offsets are supported.

The default destination pattern is `%Y-%m-%dT%H:%M:%S.%f%z`. An optional
`--format PATTERN` changes only `destination_time`. Supported directives are
`%Y %m %d %H %M %S %f %z %a %b %j %%`; other bytes are literal. `%f` omits trailing
zeros and is `0` for zero; `%z` is ±HHMM, adding SS for second-resolution offsets.
Patterns are validated before file access, even for an empty list. Invalid or
unfinished directives report their byte index. Names are fixed English std
abbreviations. JSON escaping preserves arbitrary literal pattern characters.

`grants.json` grants stdout and read access only to `fixtures/`, resolved relative
to the grants file. Set `host` to your input directory for real meetings. All
input and zone paths are basenames in the logical `input` directory: empty,
dot/parent, slash, backslash, colon, NUL and names over 128 UTF-8 bytes are refused.
There is no write, stderr, stdin, environment, process, socket, task or clock grant.
`--help` requires only stdout and does not read files.

Bounds: 65,536 bytes per JSON input and per TZif file, 64 records, 128 decoded
UTF-8 bytes per field and 128 bytes per pattern. `file.read_all_bounded` probes
one byte beyond the cap and rejects before adding that byte to the result. The
pull reader refuses row 65 before decoding it, and keeps at most four field keys
plus one unknown candidate. A temporary decoded string can occupy the bounded
input size before its field-size check. At most 64 input rows and 64 converted
rows are retained. Source/destination TZif bytes and opaque parsed rules live
only for the current conversion; their transition counts are bounded by file
bytes. Output expansion is bounded by these record, field and pattern caps.
These are work/data bounds, not an exact RSS promise.

Exit **0** means success or help; **2** means invalid input, a missing grant, or
I/O failure. Stderr is empty. The pure model's typed `InputError` retains nested
JSON, conversion, zone-resolution, parse, format and file error unions. Its
public invalid-input report has the form:

```json
{"class":"invalid_input","error":"resolve","row":1,"detail":"Gap","offset":0}
```

That is the exact result for `invalid.json`, a proposed meeting in the spring
gap without an explicit policy. `row` is one based for row errors and zero for
whole-input, path, format or I/O errors. `offset` is a JSON/pattern byte index
where supplied; parse APIs expose only `Invalid`/`OutOfRange`. Error codes are
`input_bytes`, `row_limit`, `json`, `fields`, `field_bytes`, `policy`, `path`,
`usage`, `directory_grant`, `file_open`, `file_bytes`, `file_read`, `local`,
`source_tzif`, `destination_tzif`, `resolve`, `range`, `format`. I/O details are
intentionally summarized; missing stdout or a failed output write can produce
status 2 without a report. Compiler/launcher refusals precede entry.

For native builds, configure the pinned LLVM, runtime archives and link kit as
in [Building](../../docs/building.md#what-a-native-build-needs), then:

```sh
hirsel build examples/zonetable/hirsel.json --backend llvm -o /tmp/zonetable
/tmp/zonetable --hirsel-grants examples/zonetable/grants.json success.json
# Use --backend cranelift for the other native backend.
```

The focused `zonetable_23` CLI test packages all sources/fixture bytes as declared
inputs, checks source and reachable TIR, invokes actual interpreter and both
native build/run paths, and compares exact stdout, stderr and statuses to an
independent calendar/offset oracle. It covers the Jiff before/at/after gap and
fold vectors with nanoseconds and all policies, UTC/fixed offsets, leap day,
invalid TZif/date/pattern/JSON, input/record bounds, grants, help and repeated
output. Runtime processes run sequentially through `tools/capped` at 60 s / 2 GiB;
native compilation is setup. Tests hash fixture files before/after, check child
exit statuses, remove private input/grants files and assert cleanup. A second
test executes pure parsing, gap conversion, fold rejection and format validation
under the existing checked-heap runner.

The usage and vectors port Jiff at
`fff640d47f72ca6d3a122678315bbd76488306fe`,
`crates/jiff/src/tz/ambiguous.rs:509–560` and
`crates/jiff/src/tz/timezone.rs:2656–2671` (MIT; Andrew Gallant).
[LICENSE-jiff](LICENSE-jiff) preserves the license. Deviations: explicit granted
TZif bytes replace ambient database lookup; results are table rows instead of
Rust assertions; Earlier/Later also expose the selected source civil time;
application limits, formatting and errors are added. No timezone implementation
is ported locally; all conversion uses shipped std.

`fixtures/new-york.tzif` (3,552 bytes) and `utc.tzif` (114 bytes) materialize the
existing `packages/fixtures/tzif@1/zones.hir` constants, originally Jiff test data.
`plus-five.tzif` is the 114-byte `Etc/GMT-5` slice 791..905 of that pinned Jiff
checkout's `crates/jiff-tzdb/concatenated-zoneinfo.dat` (`tzname.rs:420`). Zone data
is public-domain IANA data. `malformed.tzif` is an application-owned truncated
header. No host `/usr/share/zoneinfo` is consulted. [FRICTION.md](FRICTION.md)
records the observed authoring and setup repairs.
