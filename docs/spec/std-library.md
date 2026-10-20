# Standard library

Browse the [full signature index](../api.md) for every public core and std declaration.

The standard library is the ordinary source package `std@1`, at
`stdlib/hirsel.json`, pinned exactly by the selecting manifest
([packages](programs.md#package-selection)). Programs use short module names such as
`string.find` and `json.parse`; the catalogue uses these source spellings. Definition
identity, proof obligation ids and cache keys retain `std@1/...`. The fully versioned
spelling stays legal and is required to disambiguate multiple selected majors. Core
and std modules of the same path share a head namespace under
[LANG-4](language.md#lang-4-names). Std has no builtin bodies or privileged hooks and
uses no `Unsafe`. It raises language failures only through `failure.check`
([core library](core-library.md#memory-and-iteration)). Its algorithms follow named
reference implementations, ported with their tests.

File operations and buffered file readers use the core error type, written in programs as:

```hirsel
type FileError = FileFault(u32) | Canceled(CancelCause) | NotGranted() |
  NotFound() | PermissionDenied() | AlreadyExists() | IsDirectory() | NotDirectory()
```

Match `Error(NotFound())` for an absent file. `FileFault(code)` carries an
unclassified host error code; it is not an application input-validation error.
See the [file I/O tour](../tour.md)
for reading and atomic write-then-commit replacement.

`ascii` classifies and maps bytes (`u8`), as Rust's `u8::is_ascii_*` do; byte
literals such as `b'a'` spell its arguments, and `Bytes` iteration produces them.
Digits are `b'0'`–`b'9'`, letters are `b'A'`–`b'Z'` and `b'a'`–`b'z'`, and
alphanumeric is their union. Space means ASCII space or 0x09–0x0D, including
vertical tab and form feed. Predicates are false for bytes above 0x7F; case
conversion preserves every nonletter, including those bytes.
`digit_value` returns `Some(0)` through `Some(9)` for decimal digits and `None()`
otherwise. These are pure, nonallocating algorithms adapted from Zig's ASCII module.

## Markdown

`markdown` parses CommonMark 0.31.2 into an editable owned tree. It adapts
commonmark.js's container, bracket and emphasis passes. `Node` contains a
`kind: Kind` and `children: List[Node]`; `Kind` carries block/inline payloads
(text, heading level, list start/tightness, destinations/titles, code info,
optional task state and table alignment). Rendering reads only that tree.

```text
fn markdown.default_options() -> Options uses {}
fn markdown.gfm_options() -> Options uses {}
fn markdown.parse(take source: String, options: Options) -> Node uses {}
fn markdown.render_html(document: Node) -> String uses {}
fn markdown.to_html(take source: String, options: Options) -> String uses {}
```

`Options` has four independent booleans: `tables`, `strikethrough`, `autolinks`
and `task_lists`. All default to false; `gfm_options` enables all four. Markdown
is a total grammar: incomplete delimiters are ordinary text, CRLF/bare CR
normalize to LF, and NUL becomes U+FFFD. Named entities use a generated table
from cmark-gfm. `render_html` escapes text and attributes and emits raw HTML
nodes verbatim. List payloads hold `(ordered, start, tight)`; table cells hold
alignment `0` (none), `1` (left), `2` (center) or `3` (right). Task items hold
`None()` for an ordinary item and `Some(checked)` for a checkbox.

The generated conformance programs compare the exact published HTML for all
652 CommonMark examples and the GFM table, strikethrough, autolink and task-list
examples. Each has an independently reference-rendered truncated-input twin
with NUL and unterminated bracket/entity syntax, using the same extension
options as its published example. `tools/generate-markdown.py`
pins both upstream checkouts and regenerates entity data and vectors.

## Schema boundaries

`schema` supplies pure explicit dictionaries between inert domain values and
inert structural wire data, structured issues and shared traversal limits.
The complete contracts, ownership rules, budgets and JSON adapter guarantees
are in [pure representation boundaries](schema.md). Facts and opaque domains
are reconstructed by their owners after validation; canonical HSC1 admission
and legacy JSON derive behavior are unchanged.

## The catalogue

These declarations are normative: the source package must declare exactly these
signatures. `N` keeps the ledger's meaning of an explicit `@noalloc` promise.
`@track_caller` retains the caller attribution declared by the source helper.

```text
N fn ascii.is_digit(c: u8) -> bool uses {}
N fn ascii.is_alpha(c: u8) -> bool uses {}
N fn ascii.is_alnum(c: u8) -> bool uses {}
N fn ascii.is_space(c: u8) -> bool uses {}
N fn ascii.is_upper(c: u8) -> bool uses {}
N fn ascii.is_lower(c: u8) -> bool uses {}
N fn ascii.to_upper(c: u8) -> u8 uses {}
N fn ascii.to_lower(c: u8) -> u8 uses {}
N fn ascii.digit_value(c: u8) -> Option[u8] uses {}
N fn char.from_ascii(byte: u8) -> Option[char] uses {}
N fn char.from_u8(byte: u8) -> char uses {}
@preserves_unique(out) fn char.format_into(value: char, mut out: List[u8]) -> () uses {}
fn char.to_string(value: char) -> unique String uses {}
fn path.normalize(p: Path) -> Result[Path, path.PathError] uses {}
fn path.join(base: Path, child: Path) -> Result[Path, path.PathError] uses {}
fn path.parent(p: Path) -> Option[Path] uses {}
fn path.filename(p: Path) -> Option[unique String] uses {}
fn path.extension(p: Path) -> Option[unique String] uses {}
fn path.components(p: Path) -> unique List[String] uses {}
N fn duration.from_parts(seconds: i64, nanoseconds: i32) -> time.Duration uses {}
N fn duration.parse(text: String) -> Result[time.Duration, scalar.ParseError] uses {}
fn duration.to_string(value: time.Duration) -> unique String uses {}
N fn time.parse(text: String) -> Result[time.Timestamp, scalar.ParseError] uses {}
fn time.to_string(value: time.Timestamp) -> Result[unique String, time.TimeError] uses {}
N fn time.elapsed(clock: MonotonicClock, start: Instant of clock) -> Result[time.Duration, ClockError]
N fn time.between(clock: MonotonicClock, start: Instant of clock, end: Instant of clock) -> time.Duration
N fn time.compare(clock: MonotonicClock, a: Instant of clock, b: Instant of clock) -> i32
N fn time.deadline(clock: MonotonicClock, interval: time.Duration) -> Result[Instant of clock, time.SleepError]
N fn time.sleep(clock: MonotonicClock, interval: time.Duration) -> Result[(), time.SleepError]
fn bytes.replace(src: Bytes, old: Bytes, new: Bytes) -> unique Bytes in src
fn bytes.from_inline[n: size](xs: Inline[u8, n]) -> unique Bytes uses {}
N fn map.same_entries[K: key, V: data](a: Map[K, V], b: Map[K, V]) -> bool
@preserves_unique(m) fn map.upsert[K: key, V](mut m: unique Map[K, V], k: K, initial: fn() -> unique V uses {}, change: fn(mut value: unique V) -> () uses {}) -> () uses {}
@preserves_unique(m) fn map.update[K: key, V](mut m: unique Map[K, V], k: K, change: fn(mut value: unique V) -> () uses {}) -> bool uses {}
N fn unicode.graphemes(s: String) -> GraphemeView in s
fn json.parse(take text: String) -> Result[Value, JsonError] uses {}
fn json.stringify(value: Value) -> Result[unique String, JsonError] uses {}
@preserves_unique(out) fn json.stringify_into(value: Value, mut out: List[u8]) -> Result[(), JsonError] uses {}
fn schema.default_options() -> Options uses {}
fn schema.check_state(o: Options) -> Result[CheckState, SchemaError] uses {}
fn schema.charge_check(mut check: CheckState) -> Result[(), SchemaError] uses {}
fn schema.enter_node(mut check: CheckState) -> Result[(), SchemaError] uses {}
fn schema.leave_node(mut check: CheckState) -> () uses {}
fn schema.should_stop(check: CheckState) -> bool uses {}
fn schema.invalid( mut check: CheckState, code: String, stage: Stage, schema_site: String, expected: Option[String] ) -> SchemaError uses {}
fn schema.decode[D: inert, W: data + inert](b: Boundary[D, W], wire: W, o: Options) -> Result[ D, SchemaError ] uses {}
fn schema.encode[D: inert, W: data + inert](b: Boundary[D, W], value: D, o: Options) -> Result[ W, SchemaError ] uses {}
fn schema.decode_only[D: inert, W: data + inert](b: Decoder[D, W], wire: W, o: Options) -> Result[ D, SchemaError ] uses {}
fn schema.then[A: inert, B: data + inert, W: data + inert]( take outer: Boundary[B, W], take inner: Boundary[A, B] ) -> Boundary[A, W] uses {}
fn schema.prefix(segment: Segment, take issues: Issues) -> Issues uses {}
fn schema.prefix_error(segment: Segment, take error: SchemaError) -> SchemaError uses {}
fn schema.empty_issues() -> Issues uses {}
fn schema.append_issues(mut into: Issues, take more: Issues) -> () uses {}
fn schema.format(issues: Issues) -> String uses {}
fn schema.identity[W: plain + data + inert]() -> Boundary[W, W] uses {}
fn schema.u8_json() -> Boundary[u8, std@1/json.Value] uses {}
fn schema.text_json() -> Boundary[String, std@1/json.Value] uses {}
fn schema.list[D: inert, W: data + inert](take item: Boundary[D, W]) -> Boundary[ List[D], List[W] ] uses {}
fn schema.array_json() -> Boundary[List[std@1/json.Value], std@1/json.Value] uses {}
fn schema.list_json[D: inert](take item: Boundary[D, std@1/json.Value]) -> Boundary[ List[D], std@1/json.Value ] uses {}
fn schema.parse_json[D: inert]( b: Boundary[D, std@1/json.Value], input: String, o: Options ) -> Result[D, BoundaryError] uses {}
fn schema.write_json[D: inert](b: Boundary[D, std@1/json.Value], value: D, o: Options) -> Result[ String, BoundaryError ] uses {}
fn msgpack.decode(data: Bytes) -> Result[json.Value, binary.DecodeError] uses {}
fn msgpack.encode(value: json.Value) -> Result[unique Bytes, binary.DecodeError] uses {}
fn cbor.decode(data: Bytes) -> Result[json.Value, binary.DecodeError] uses {}
fn cbor.encode(value: json.Value, deterministic: bool) -> Result[unique Bytes, binary.DecodeError] uses {}
fn yaml.default_config() -> yaml.Config uses {}
fn yaml.parse_with(take text: String, config: yaml.Config) -> Result[yaml.Value, yaml.YamlError] uses {}
fn yaml.parse_all_with(take text: String, config: yaml.Config) -> Result[List[yaml.Value], yaml.YamlError] uses {}
fn yaml.document_with(take text: String, config: yaml.Config) -> Result[yaml.Document, yaml.YamlError] uses {}
fn yaml.parse(take text: String) -> Result[yaml.Value, yaml.YamlError] uses {}
fn yaml.parse_all(take text: String) -> Result[List[yaml.Value], yaml.YamlError] uses {}
fn yaml.stringify(value: yaml.Value) -> Result[unique String, yaml.YamlError] uses {}
fn yaml.document(take text: String) -> Result[yaml.Document, yaml.YamlError] uses {}
fn yaml.source(doc: yaml.Document) -> String in doc uses {}
fn yaml.root(doc: yaml.Document) -> yaml.Value uses {}
fn yaml.get(doc: yaml.Document, path: List[yaml.Segment]) -> Option[yaml.Value] uses {}
fn yaml.set(mut doc: yaml.Document, path: List[yaml.Segment], value: yaml.Value) -> Result[(), yaml.YamlError] uses {}
fn yaml.remove(mut doc: yaml.Document, path: List[yaml.Segment]) -> Result[(), yaml.YamlError] uses {}
fn toml.parse(take text: String) -> Result[toml.Value, toml.TomlError] uses {}
fn toml.stringify(value: toml.Value) -> Result[unique String, toml.TomlError] uses {}
fn toml.document(take text: String) -> Result[toml.Document, toml.TomlError] uses {}
fn toml.source(doc: toml.Document) -> String in doc uses {}
fn toml.root(doc: toml.Document) -> toml.Value uses {}
fn toml.get(doc: toml.Document, path: List[toml.Segment]) -> Option[toml.Value] uses {}
fn toml.set(mut doc: toml.Document, path: List[toml.Segment], value: toml.Value) -> Result[(), toml.TomlError] uses {}
fn toml.remove(mut doc: toml.Document, path: List[toml.Segment]) -> Result[(), toml.TomlError] uses {}
fn toml.moment(spelling: String) -> Result[toml.Moment, toml.TomlError] uses {}
fn toml.datetime(value: toml.Moment) -> Result[toml.Value, toml.TomlError] uses {}
@track_caller N fn test.equal[T: data](t: Test, actual: T, expected: T) -> ()
@track_caller N fn test.approx(t: Test, actual: f64, expected: f64, absolute: f64, relative: f64) -> ()
@track_caller fn test.assert_eq[T: data](t: Test, actual: T, expected: T, show: fn(T) -> String) -> ()
@track_caller fn test.property[T](t: Test, gen: fn(mut test.Source) -> T, law: fn(T) -> bool uses {}, show: fn(T) -> String) -> ()
@track_caller fn test.expect_error[T, E](t: Test, take value: Result[T, E]) -> E
fn input.read_all(s: Stdin) -> Result[unique Bytes, InputError]
fn input.read_line(s: Stdin) -> Result[Option[unique String], InputError]
N fn prng.seed(value: u64) -> prng.Generator uses {}
N fn prng.u64(mut generator: prng.Generator) -> u64 uses {}
N fn prng.below(mut generator: prng.Generator, upper_exclusive: u64) -> Result[u64, prng.SampleError] uses {}
fn prng.bytes(mut generator: prng.Generator, count: u64) -> unique Bytes uses {}
fn random.u64(r: Random) -> Result[u64, RandomError]
fn random.below(r: Random, upper_exclusive: u64) -> Result[u64, random.SampleError]
N fn process.pipefail(pipeline: PipelineOutput) -> ExitStatus uses {}
```

Modules beyond the catalogue (`list`, `string`, `text`, `net`, `uri`, `http_wire`, `http`,
`hpack`, `http2_wire`, `http2`, `bufio`, `fnv`, `derive`, `keys`, `task` and the
`format_into` and NFC functions below) define their contracts by
their declared signatures and the rules in this part.

## Conventions

Sibling operations share one shape per concept, so a call written from one function's
signature works for the others:

- A read that can reach the end of its input returns `Option`, `None()` at EOF:
  `input.read_line`, every `bufio` `read_line` and `csv.read_record` return
  `Result[Option[T], E]`, so `while let Some(x) = ..?` consumes them. Byte-count reads
  (`read`, `read_until`, `read_more`) return the count, zero at EOF, as core `file.read` does.
- An absent result is `None()`, never a sentinel such as `""` or an out-of-range index
  (`path.parent`, `path.filename`, `path.extension`, `string.find`, `list.find`, `map.get`);
  `Result` carries only failures.
- A query takes the container first and the key or pattern after it; a module's own
  type comes first (`map.get(m, k)`, `string.find(s, pattern)`, `regex.find(re, text, ..)`).
- A function that appends to a caller's builder is the returning function's name plus
  `_into`, with the builder `mut out` last (`json.stringify` and `json.stringify_into`).
- A value renders to text with `to_string(value) -> String` and appends that text with
  `format_into(value, mut out)`; a scalar's fixed-buffer form is `format_inline`. The name
  `format` is kept for rendering under an explicit pattern or layout, which is its second
  parameter. F-strings call the same `format_into` and `format_into_spec` names.
- An element count is `len`; a predicate is named `is_..` where it shares a concept with
  another module (`regex.is_match`, `glob.is_match`), and equivalent draws share names
  across sources (`random.u64`, `random.below`, `random.bytes` and `prng.u64`,
  `prng.below`, `prng.bytes`).

## Option and Result helpers

`option` and `results` adapt Rust core Option/Result helpers. Queries borrow;
consuming helpers take the carrier and callbacks, and `filter` borrows its payload
for the predicate. The consuming helpers are parametric calls
([MODE-2](ownership.md#mode-2-uniqueness)), so a unique payload stays unique through them,
except `filter`, which lends its payload to the predicate. Rust `is_err` / `err` / `map_err` use `is_error` / `error` /
`map_error`. The module is `results` because `result` is reserved. Panicking
`unwrap` / `expect` are omitted. `option.remove(mut value)` adapts Rust
`Option::take`: it returns the current option and leaves `None` in its place,
using core `replace`. A unique payload remains unique.

## Task combinators

`task.all` and `task.select` (module `task`) are ordinary source over the core task-set
operations ([CONC-2](concurrency.md#conc-2-tasks)), which define their behaviour. Both take
the set, wait in completion order and cancel and join every member they leave before
returning, so no member outlives the call.

`task.map_all(tasks, take items, grain, take f)` opens a nested scope and maps
each consecutive group of at most `grain` items on one child. `f` is an inert owned
callable with `uses {}` and returns `Result[U, E]`. Inputs and results are
copyable and inert, and errors are sendable. Results are flattened
in input order through `task.all`; the first completed error cancels and joins
the remaining children. Empty input returns `Ok([])`. Grain must be positive
(zero fails SEM-1). Grain is a program choice, independent of execution width.

## Actors

`actor.Actor[M]` is an opaque resource wrapping a `Sender[M]`.
`actor.spawn(s, tasks, take initial, take handle)` starts one task in `s`,
owning `unique S` and a mailbox receiver. The sendable handler takes a message
and borrows the state by `mut unique S`; it may use I/O. `actor.dup` acquires
another sender. Releasing every handle drains the mailbox and ends the task;
an error or failure exit cancels and joins it. A normal scope exit waits
for the actor, so its mailbox handles must be released first.

`actor.cast` sends a message and returns `Result[(), (M, ChannelError)]`.
Like `channel.send`, it never checks cancellation. `actor.call` sends a
request constructed by a taken once callback receiving a taken `actor.Reply[R]`,
then waits for one reply. It returns `Result[R, ChannelError]`; an abandoned
reply or closed actor returns `Closed`, and waiting may return `Canceled`.
`Reply[R]` is a sender alias. `actor.reply` consumes it, returning the value
on a failed send. These are ordinary source wrappers over channels, with no
runtime actor operations. See the [agent guide](../concurrent-agents.md).

## Paths

`path.PathError = Invalid(u64)` reports a byte offset. Components are separated by
slashes only, and empty components are ignored. `normalize` removes `.` components and
cancels `..` against an ordinary preceding component, preserves leading relative `..`,
rejects escaping above an absolute root and rejects NUL and backslash. An empty relative
result is `.`; an absolute empty component stack is `/`. `join` requires a relative child,
rejects NUL and backslash in both arguments and normalizes the combined path; an empty base
contributes no separator. `parent`, `filename` and `components` inspect the raw nonempty
slash components, ignoring trailing slashes, without normalizing. Empty and root paths have
no parent or file name (`None()`); a single relative component has parent `.`, and a single
absolute component has parent `/`. `extension` is the last non-leading dot suffix of the file
name: `None()` without one (`a`, `.hidden`), `Some("")` for an empty suffix (`a.`). Absence is
`None()` throughout, never an empty string. These functions take no `Dir`, touch no
host, resolve no links and establish no confinement.

## Durations and timestamps

`time.Duration` holds signed `i128` nanoseconds. `duration.from_parts` widens signed
`i64` seconds and signed `i32` nanoseconds before summing, with no range imposed on the
fractional part. `duration.parse` accepts an optional sign, integer decimal seconds, an
optional dot followed by one to nine fraction digits, and a final `s`, with no whitespace.
Invalid grammar is `scalar.ParseError.Invalid`; a value not representable as signed
`i128` nanoseconds is `OutOfRange`. `duration.to_string` covers the whole signed `i128` range
and emits canonical seconds with a final `s` and trimmed fractional zeros; zero is `0s`.

`time.Timestamp` holds signed `i128` `unix_nanoseconds`; `TimeError` is `OutOfRange`.
`time.parse` accepts RFC 3339 with Gregorian years 0000–9999, a `T` or `t` separator, a `Z`,
`z` or numeric `±HH:MM` offset (hours 00–23, minutes 00–59), and an optional fraction of one
to nine digits, validating dates and 00–23/00–59/00–59 times. Space separators, a leap
second 60, a missing offset and trailing data are invalid. Numeric offsets normalize to UTC,
`-00:00` included; an offset may move the UTC result outside the formatting range.
`time.to_string` returns UTC with a four-digit year, uppercase `T` and `Z` and trimmed
fractional zeros, or `TimeError.OutOfRange` outside years 0000–9999. These are ordinary
algorithms with no clock authority, locale or time-zone database.

`time.Duration` is the only duration type. `time.elapsed`, `time.between` and
`time.compare` work on `Instant`s of one `MonotonicClock`; their brand ties both instants
to that clock, and `time.between` subtracts nanosecond ticks.

`time.deadline(clock, d)` returns the instant `d` after a fresh sample of `clock`, and
`time.sleep(clock, d)` waits until that instant with `clock.sleep_until`. A negative `d` is
`NegativeDuration()` and a deadline that does not fit the clock's `u64` ticks is
`OutOfRange()`, both before any wait; a failed sample is `Wait(Clock(e))` and a wait's own
error is `Wait(e)`. `time.SleepError` is `NegativeDuration() | OutOfRange() |
Wait(WaitError)`. Retrying with a delay is a loop around `time.sleep`; nothing polls.

## Civil dates and times

Civil values carry no time zone. They are ordinary algorithms adapted from jiff's
`civil` module: no clock authority, locale or time-zone database, and no implicit local
zone. Each type keeps one module of functions named after it, as `time.Duration` keeps
`duration`: `date`, `time_of_day` and `date_time`, and `time_zone` for `time.TimeZone`
(see Time zones).

`time.Date` is a proleptic Gregorian date of years -9999 through 9999 (jiff's range);
`time.TimeOfDay` is an hour 0–23, minute 0–59, second 0–59 and nanosecond 0–999999999,
with no leap second. Both are opaque, `plain + key`, so `==` compares them and they are
map keys; every value is valid because `date.new(y, m, d)` and
`time_of_day.new(h, m, s, ns)` return `None()` for impossible fields (month 13, February
29 of a common year, hour 24). `time.DateTime { date, time }` is a transparent record of
the two. The type is `TimeOfDay`, not `Time`, because a Go-trained reader takes `time.Time`
for an instant. Accessors read the fields (`date.year`, `date.month`, `date.day`,
`time_of_day.hour` and so on), and `time_of_day.midnight()` is 00:00:00.

Day counts use Howard Hinnant's `days_from_civil` and `civil_from_days`, so
`date.to_epoch_days` is days since 1970-01-01 and `date.from_epoch_days` inverts it
within the range (`None()` outside). A year is a leap year when divisible by 4 and not by
100, or divisible by 400; year 0 is a leap year. `date.weekday` returns `time.Weekday`
(`Monday` through `Sunday`). `date.ordinal` is the day of the year, 1 through 365 or 366,
and `date.from_ordinal(y, n)` inverts it. `date.iso_week` returns the ISO 8601
`time.IsoWeek { year, week }`: week 1 holds the year's first Thursday, so a date in early
January can belong to the previous ISO year and late December to the next; a year has 53
weeks when it starts on a Thursday, or on a Wednesday in a leap year.
`date.from_iso_week(y, week, weekday)` inverts it and is `None()` for a week the year lacks.

Arithmetic is checked. `date.add_days(d, n)` is `TimeError.OutOfRange` past the range.
`date.add_months(d, n, overflow)` and `date.add_years(d, n, overflow)` move the calendar
month (twelve per year) and keep the day of the month; when that day is past the target
month's end, `time.Overflow` decides: `Clamp` uses the month's last day (January 31 plus
one month is February 28 or 29; February 29 plus one year is February 28), and `Reject`
returns `CalendarError.NoSuchDay`. A target year outside the range is
`CalendarError.OutOfRange` under either policy. Adding months then days is not adding days
then months; nothing reorders the steps. `date.days_between(start, end)` is `end - start` in
days. `date.compare`, `time_of_day.compare` and `date_time.compare` return -1, 0 or 1 in
chronological order.

`date.to_string` writes ISO 8601 `YYYY-MM-DD` for years 0000–9999 and otherwise the
expanded form with a sign and six year digits (`-000044-03-15`); `date.parse` accepts
exactly those two forms (a `+` six-digit form included), rejecting `-000000`, and a year
beyond 9999 is `scalar.ParseError.OutOfRange`. `time_of_day.to_string` writes `HH:MM:SS`
with a dot and the fraction's digits when it is nonzero, trailing zeros trimmed;
`time_of_day.parse` accepts `HH:MM` or `HH:MM:SS` with an optional dot and one to nine
fraction digits. `date_time.to_string` joins them with `T`, and `date_time.parse` accepts
`T` or `t` there and no offset. All three types format through `@format`, so f-string holes
render the same text. Any other text is `Invalid`.

`Date` and `TimeOfDay` are serializable through their checks
([SER-1](serialization.md#ser-1-serializable-schemas)): each encodes as its fields
(`year: i32, month: u8, day: u8` and `hour: u8, minute: u8, second: u8, nanosecond: u32`),
and decoding admits exactly the fields `date.new` and `time_of_day.new` admit, otherwise
`DecodeError.Invalid`. So `DateTime`, `BrokenDownTime` and `toml.Moment` are serializable.

A fixed offset is `i32` seconds east of UTC, valid strictly inside ±26 hours (jiff's
±25:59:59); any other offset is `TimeError.OutOfRange`. `date_time.from_timestamp(ts, offset)`
is the civil datetime of `ts` at `offset`, `OutOfRange` when that lies outside the date
range, and `date_time.to_timestamp(dt, offset)` is its inverse. `date_time.add(dt, d)` and
`date_time.between(start, end)` work on the zone-free civil timeline, where every day has
86400 seconds. `time.to_string_at(ts, offset)` is RFC 3339 with that offset written
`±HH:MM` (`+00:00` for zero) and the time written as `time.to_string` does; the offset must
be whole minutes under 24 hours and the local year 0000–9999, otherwise `OutOfRange`, so
the text always parses back to `ts` with `time.parse`.

`time.format(value, pattern)` and `time.parse_with(text, pattern)` adapt jiff's
`strtime` over `time.BrokenDownTime { datetime, offset: Option[i32] }`; `time.broken_down(ts,
offset)` builds one from a timestamp. As std names it, `format` always takes a pattern as
its second argument and `to_string` takes none. Pattern bytes other than `%` are copied or,
when parsing, matched exactly. The directives are:

| Directive | Formats | Parses |
|---|---|---|
| `%Y` | year, zero-padded to four characters including a `-` sign (`0024`, `-024`, `-2025`) | optional sign and one to four digits |
| `%m` `%d` `%H` `%M` `%S` | two digits | one or two digits; `%S` 60 reads as 59 |
| `%j` | day of the year, three digits | one to three digits |
| `%f` | fraction digits without the dot, trailing zeros trimmed, `0` for zero | one to nine digits |
| `%z` | `±HHMM`, then `SS` when the offset has seconds | `±HHMM` or `±HHMMSS`, hours at most 25 |
| `%a` `%b` | English weekday and month abbreviation (`Sun`, `Jul`) | the same, in any ASCII case |
| `%%` | `%` | `%` |

`time.format` returns `FormatError.Invalid(i)` for an unknown or unterminated directive at
pattern byte `i`, `NoOffset` for `%z` without an offset and `OutOfRange` for an offset
outside the valid range. `time.parse_with` returns `scalar.ParseError.Invalid` for a pattern
error or input that does not match, including trailing input. Its date needs `%Y` with `%m`
and `%d`, or `%Y` with `%j`; when several are present they must agree, as must `%a`. Absent
time fields are zero, but a minute needs an hour, a second a minute and a fraction a second.
The offset is `Some` exactly when the pattern has `%z`. Flags, widths and other directives
are not supported.

## Time zones

`time.TimeZone` is a zone's rules, read from TZif data (RFC 8536, versions 1 through 4)
that the program supplies, for example bytes read from `/usr/share/zoneinfo` through a
granted `Dir` or embedded in the program. There is no ambient database. Lookup follows
jiff's `tz` module: before the first transition the first local time type applies; at
or after the last one the version 2+ footer's POSIX TZ rule decides, or, with an empty
footer, the last transition's offset holds. `time_zone.utc()` and `time_zone.fixed(offset)`
are zones with one offset forever; `fixed` returns `TimeError.OutOfRange` for an offset
outside ±26 hours.

`time_zone.from_tzif(data)` returns `scalar.ParseError.OutOfRange` for a local time type
whose offset is outside ±26 hours and `Invalid` for any other malformed data: a wrong
magic or truncated block, unequal indicator and type counts, no local time type, a
transition naming a missing type, decreasing transition times, a designation index past
the table or without a NUL, an indicator other than 0 or 1 or a UT indicator on a wall
time, a footer not enclosed in newlines or longer than 1024 bytes, a malformed POSIX rule,
or a rule whose offset or DST flag at the last transition differs from that transition's.
Transition times are clamped to the date range, and leap-second records are skipped.
Abbreviations are validated but not kept.

The footer rule is POSIX TZ with the IANA version 3 extensions: names of three or more
letters, or `<...>` quoted names of letters, digits, `+` and `-`; offsets `[+-]hh[:mm[:ss]]`
with hours 0–24, west of UTC as POSIX writes them; DST one hour ahead of standard time
unless given; rule days `Jn` (1–365, February 29 never counted), `n` (0–365) and `Mm.w.d`
(week 5 is the month's last); rule times from -167 to 167 hours, 02:00 when absent. A rule
day that does not exist in a year (`365` in a common year) and a rule time that leaves its
year clamp to that year's start or end, and DST that would end where the next year's
begins is permanent (jiff `regression_permanent_dst`).

`time_zone.offset(zone, ts)` is the offset in seconds east of UTC at `ts`, and
`time_zone.to_datetime(zone, ts)` the civil datetime the zone shows then, `OutOfRange`
outside the date range. `time_zone.to_timestamp(zone, dt, disambiguation)` is the instant
the zone shows as `dt`. A transition to a larger offset skips civil times (a gap) and one
to a smaller offset repeats them (a fold); `time.Disambiguation` picks among them as jiff's
`AmbiguousTimestamp` does:

| `Disambiguation` | Gap | Fold |
|---|---|---|
| `Earlier` | the offset after the gap, so the earlier instant | the first occurrence, in the offset before |
| `Later` | the offset before the gap, so the later instant | the second occurrence, in the offset after |
| `Reject` | `ZoneError.Gap` | `ZoneError.Fold` |

No instant shows a civil time in a gap: `Earlier` returns the instant the zone shows as
that time less the gap's length and `Later` the one it shows as that time plus it (01:30
and 03:30 for 02:30 in a one-hour gap at 02:00). Every other civil time names one instant,
which all three policies return.

## JSON

```text
Value     = Null() | Bool(bool) | Number(String) | Text(String)
          | Array(List[Value]) | Object(Map[String, Value])
JsonError = Syntax(u64) | DuplicateKey(u64) | Nesting(u64) | InvalidNumber()
```

Arrays and objects hold `Value`s directly ([MEM-7](memory.md#mem-7-layout)): a member
selected with `list` or `map` operations is a `Value`, which passes straight to
`json.stringify` or `json.stringify_into`.

`json.parse` takes a UTF-8 `String` and returns an owned tree, keeping validated decimal
number lexemes exactly. It accepts JSON whitespace only, validates escaped UTF-16 surrogate
pairs, rejects duplicate decoded object keys and preserves insertion order. `Syntax`,
`DuplicateKey` and parser `Nesting` payloads are zero-based byte offsets into the input; end
of input is its byte length, and `DuplicateKey` points at the repeated key's opening quote.
The constant `MAX_NESTING: u64 = 128` counts open arrays and objects, scalars using no
level; opening container 129 is `Nesting`. A partial tree is released through ordinary error
cleanup.

`json.stringify` borrows a `Value` and returns compact UTF-8 JSON; `json.stringify_into` borrows it
and appends to a mutable builder, keeping existing content and possibly leaving a written
prefix on error. Both validate every `Number` against
`-?(0|[1-9][0-9]*)(\.[0-9]+)?([eE][-+]?[0-9]+)?`, requiring complete consumption, and return
`InvalidNumber` otherwise. They preserve number spelling and object order, escape ASCII
controls, quotes and backslashes, and leave other scalars unescaped. Writing uses the same
nesting bound, returning `Nesting(128)` at the attempted container depth. This is library
policy, independent of the depth limits of [MEM-3](memory.md#mem-3-freeing), with no
reflection, automatic record conversion or authority.

## MessagePack and CBOR

`msgpack` implements the MessagePack JSON domain; `cbor` implements the corresponding
RFC 8949 domain. Both decode exactly one item into `json.Value`, reject trailing bytes,
validate UTF-8, reject duplicate text map keys, and preserve insertion order. Integers
become exact decimal `Number` lexemes, including MessagePack's unsigned 64-bit maximum
and CBOR's negative minimum, -18446744073709551616. Finite binary16/32/64 floats become
shortest binary64 decimal text, with signed zero preserved and `.0` appended when
needed to retain the floating-point type (`1.0` stays a float on re-encoding). A
subsequent encode chooses integer or floating representation from that text; original
wire widths are not retained.

There is no lossless JSON mapping for binary strings, MessagePack extensions, CBOR tags,
nonfinite floats, non-text keys or CBOR simple values other than bool/null. These return
`Unsupported`; they are never silently converted to strings, arrays or null. CBOR accepts
definite and indefinite text, arrays and maps, and nonpreferred integer/float widths.
Reserved two-byte simple values below 32 are malformed and return `Syntax`.
Every indefinite text chunk must independently be valid UTF-8; nested indefinite text
chunks and breaks outside indefinite containers are malformed.

Both encoders validate public `Number` strings with JSON's grammar. Integer lexemes must
fit the format's integer range; decimal/exponent lexemes convert to finite binary64.
Out-of-range integers, nonfinite conversions and invalid lexemes return `InvalidNumber`.
Negative zero is encoded as a float. Other float spellings can change on decode; JSON's
arbitrary decimal precision and spelling are not a wire-format guarantee.

MessagePack writes shortest integer and length headers and binary64 floats. String, array
or map extents above 4294967295 return `Unsupported(extent)` on encoding. CBOR always
writes definite lengths and shortest integer/length headers. With `deterministic = false`,
it retains object insertion order and uses binary64 floats. With `true`, it uses the
shortest exactly preserving float width, including signed zero, and orders text keys by
unsigned lexicographic comparison of their complete preferred CBOR encodings (RFC 8949
section 4.2.1). For this text-key domain, that is UTF-8 byte length followed by byte order.
Deterministic output is stable across input map insertion order and run hash seeds.

`binary.DecodeError` has these cases:
`Truncated(u64)`, `Syntax(u64)`, `Unsupported(u64)`, `Utf8(u64)`, `DuplicateKey(u64)`,
`Nesting(u64)`, `Trailing(u64)` and `InvalidNumber`. Decode offsets are zero-based byte
positions; UTF-8 errors identify the offending string or chunk payload start. Duplicate
errors identify the repeated key header. Both enforce the same library nesting policy as
JSON: at most 128 open arrays/maps, with scalar leaves consuming no level. Encoder nesting
errors identify the attempted depth, 128. Partial results are released on error.

These are ordinary source codecs over explicit values. Record/variant conversion can use
the existing `derive.json_value` and `derive.from_json` functions; the canonical
`codec.encode/decode[T: serializable]` envelope remains a separate wire format.

## YAML

```text
Value     = Null() | Bool(bool) | Integer(i64) | Float(f64) | Text(String)
          | Array(List[Value]) | Object(Map[String, Value])
          | Tagged(tag: String, value: box Value)
Config    = { merge_keys: bool }
Position  = { line: u64, column: u64 }
YamlError = Syntax(Position) | DuplicateKey(Position) | Nesting(Position)
          | Aliases(Position) | Tag(Position) | ComplexKey(Position)
          | Documents(Position) | Merge(Position) | InvalidValue() | NotFound() | NotCollection()
Segment   = Key(String) | Index(u64)
```

`yaml.parse_all` takes a UTF-8 `String` holding a YAML 1.2.2 stream and returns its
documents in order; `yaml.parse` returns the only document, `Null` for a stream with
none, and `Documents` at the start of a second one. Both accept the whole syntax: block
and flow collections, plain, single-quoted, double-quoted, literal and folded scalars
with every chomping and indentation indicator, explicit and implicit keys, anchors and
aliases, `%YAML` and `%TAG` directives, explicit document markers and a leading byte
order mark. Line breaks are LF, CR or CRLF and decode as LF. Text outside the printable
set (YAML 1.2.2 5.1: a control other than tab, line feed and carriage return, DEL, a C1
control other than NEL, U+FFFE or U+FFFF) is `Syntax`.

Untagged scalars resolve by the core schema (YAML 1.2.2 10.3). An untagged plain
scalar is `Null` when it is empty, `~`, `null`, `Null` or `NULL`; `Bool` for `true`,
`True`, `TRUE`, `false`, `False`, `FALSE`; `Integer` for `[-+]?[0-9]+`, `0o[0-7]+` or
`0x[0-9a-fA-F]+`; `Float` for `[-+]?(\.[0-9]+|[0-9]+(\.[0-9]*)?)([eE][-+]?[0-9]+)?`,
`[-+]?(\.inf|\.Inf|\.INF)` or `.nan`, `.NaN`, `.NAN`, rounded correctly to binary64;
and `Text` otherwise. YAML 1.1 spellings are text: `yes`, `no`, `on`, `off`, `y`, `n`,
`0b101`, `1_000`, `012` (the decimal 12), `1:30` and dates stay as written, so a GitHub
workflow's `on:` key is the string `on`. An integer outside `i64` is `Syntax` at the
scalar. A quoted or block scalar is `Text`. Core tags resolve as follows: `!` and
`!!str` make a scalar `Text`; `!!null`, `!!bool`, `!!int` and `!!float` require a
scalar of that type's spelling above; `!!seq` and `!!map` (or `!`) may tag a sequence
or mapping; `%TAG` handles resolve before this check. A core tag on content it does
not match, or an empty verbatim tag, is `Tag` at the node. Every other tag becomes
`Tagged`, keeping the resolved tag and the node's content: `Text` for a scalar,
regardless of its spelling, or `Array`/`Object` with normally resolved children for a
collection. For example, `!Ref b` is `Tagged(tag: "!Ref", value: box Text("b"))`.

Merge keys apply by default, following the YAML 1.1 merge type. A plain untagged `<<`,
or a scalar `<<` explicitly tagged `!!merge`, inserts entries from its mapping value
or sequence of mappings (including aliases). Explicit entries win wherever they
appear; among merge sources, the earlier mapping wins. Values are taken whole, with
no recursive mixing of mappings. Missing entries appear in source order at the merge
key's position, while explicit entries keep their document order. A non-mapping
source, or a non-mapping item in a source sequence, is `Merge` at the source; a second
merge key is `DuplicateKey`. A quoted `<<` or one tagged `!!str` is an ordinary key.
`yaml.default_config()` returns `Config(merge_keys: true)`. The `parse_with`,
`parse_all_with` and `document_with` functions accept this record explicitly;
`Config(merge_keys: false)` treats merge keys as ordinary string keys for strict
YAML 1.2 behavior. Documents retain the selected config through edits.

A mapping becomes an `Object` in document order. A key is a scalar, or an alias of one,
and is its content text whatever its style, type or custom tag (a key's tag is
discarded), so `!foo a` and `a` are the same key, and `1`, `'1'` and `"1"` are the same
key and a repeat of any key is `DuplicateKey` at the repeated key. A sequence or mapping
key (or an alias of one) is `ComplexKey`. Anchors are scoped to their document; a later
anchor of the same name replaces the earlier one, and an alias with no anchor before it
is `Syntax`. An alias copies its anchored node. The constant `MAX_ALIAS_NODES: u64 =
100000` bounds the nodes that aliases add to one document (each scalar, sequence,
mapping and key counts one), against exponential expansion; the alias that would exceed
it is `Aliases`. The constant `MAX_NESTING: u64 = 128` counts open sequences and
mappings, flow or block, scalars using no level; the collection that would be level 129
is `Nesting` at its first byte, and so is an alias whose anchored tree would reach that
level.

Every error that has a `Position` carries the one-based line and the one-based Unicode
scalar column (a tab counts one column) where the scanner, parser or composer detected
it; end of input is past the last scalar. A node's position is that of its first
property (anchor or tag) or, without one, of its content. These positions and the
grammar follow saphyr's port of libyaml as checked by yaml-test-suite: every case of
its 2022-01-17 data release runs in `conformance/SEM/SEM-3-yaml-test-*-run.hir`, and each
case that a `Value` cannot hold (a complex key or repeated key text, including two
empty keys) is held to the error above. Tagged cases are checked against their
resolved event trees and round-tripped; merge vectors also adapt go-yaml v3's tests.

`yaml.stringify` borrows a `Value` and writes one document in canonical block style,
ending in a line break. Mapping entries are `key: value` and sequence entries `- value`;
a nested non-empty collection starts on the next line, indented two spaces from its key
or dash, except that a collection inside a sequence entry starts on the dash's line
(`- a: 1`, `- - x`). Empty collections are `[]` and `{}`. `Null` is `null`, booleans
`true` and `false`, integers decimal, floats the shortest round-trip digits of the `f64`
module with `.0` appended to whole numbers, and `.inf`, `-.inf` and `.nan`. A string, key
or value, is plain when it reads back as the same string: non-empty, not starting with
an indicator (`-?:,[]{}#&*!|>'"%@` or backquote), a blank or `...`, not ending with a
blank or `:`, without `: ` or ` #`, without the characters double quotes escape below,
not `<<`, not a core-schema null, bool, int or float spelling, and not a YAML 1.1 boolean (`y`,
`n`, `yes`, `no`, `on`, `off` in lower, capitalized or upper case) or sexagesimal number
(a digit, after an optional sign, then digits, `_`, `.` and at least one `:`), so YAML 1.1
readers keep it a string. A non-key string with a line
break whose first line is non-empty and starts with no blank, with no line of blanks
only and no escaped character other than tab, is a literal block scalar (`|`, or `|-`
without a final line break, or `|+` with more than one). Every other string is
double-quoted, escaping `"`, `\`, `\0 \a \b \t \n \v \f \r \e`, other C0 controls and
DEL as `\xXX`, C1 controls as `\xXX` (NEL as `\N`), U+2028 and U+2029 as `\L` and
`\P`, and U+FEFF, U+FFFE and U+FFFF as `\uXXXX`. A key longer than 1024 bytes as
written (plain or quoted) becomes an explicit `? key` entry, with its `:` on the next
line. `Tagged` writes its tag before its content: `!!suffix` for a tag under
`tag:yaml.org,2002:`, `!suffix` for a local tag and `!<tag>` otherwise. Percent signs
and bytes outside the tag character set are `%XX` escaped. A tagged non-empty
collection starts on the next line. A tag must be nonempty, non-core and different
from `!`, and its content must be text or a collection; other constructed `Tagged`
values cannot read back unchanged and are `InvalidValue`. A collection that would
be level 129 is also `InvalidValue`. The output parses to an equal value, NaN to NaN.

`yaml.document` parses a one-document stream like `yaml.parse` and keeps its text;
`yaml.source` returns the current text, `yaml.root` the value and `yaml.get` the value
at a path, if any. A path's `Key` selects a mapping member and `Index` a sequence item,
looking through a collection's tag; the selected value keeps its own tag.
Edits never re-render the document: each splices the text and reparses it, so every
byte outside the edited span (comments, blank lines, key order, quoting, number
spellings, anchors and tags on kept nodes) is unchanged, and added lines use the
document's first line break (LF when it has none). An edit whose result does not parse,
or does not hold the requested value at `path` (removing an anchor that an alias still
uses, say), is `InvalidValue`; a failed edit leaves the document unchanged.

`yaml.set(mut doc, path, value)` with an empty path, or on an empty document, replaces
the text with `yaml.stringify(value)`. Otherwise:

- an existing node keeps its anchor and core tag. For example, setting `a: !!str x`
  to `Text("z")` keeps `!!str`, while setting it to `Integer(1)` is `InvalidValue`.
  A non-core tag comes from the replacement value: setting `a: !Ref b` to `Text("z")`
  writes `a: z`; a `Tagged` value with tag `!Sub` writes `a: !Sub z`. In a
  flow collection the value is written in flow style (`[a, b]`, `{k: v}`, scalars plain
  when that is safe there, else double-quoted). In a block collection a scalar or empty
  collection is written on the node's line, keeping any comment after it, and a
  non-empty collection is written in canonical block style below its key or dash,
  indented two spaces further, the comment staying on the key's line; multi-line text
  is a literal block unless something other than blanks follows the replaced node on
  its line, when it is double-quoted;
- a missing key of a block mapping becomes `key: value` on a new line after the
  mapping's last entry, at its entries' indentation; `Index(len)` on a block sequence
  adds `- value` the same way; in flow collections the entry is added after the last
  one (`, key: value`) or inside empty brackets; other missing indices are `NotFound`;
- missing mappings along the path are created by adding their first key as a mapping
  holding the rest of the path; a missing `Index` along the path is `NotFound`;
- a key inherited only from a merge becomes an explicit entry, holding the inherited
  value with the remaining path set. The anchor and merge source are unchanged.

A path through a scalar or an alias, a `Key` on a sequence or an `Index` on a mapping is
`NotCollection`. `yaml.remove(mut doc, path)` removes the value at `path`, or returns
`NotFound` (always for the empty path). A block entry takes its lines and, after the
first entry, the comment and blank lines back to the previous entry; the first entry
takes the text up to the second, so a compact `- a: 1` entry keeps its dash. A flow
entry takes the comma after it, or the last one the comma before it. Removing the only
entry of a collection leaves it empty, written `{}` or `[]` in its place, keeping
its custom tag. A key inherited only from a merge cannot be removed without changing
the merge source and returns `InvalidValue`. Removing an explicit override that would
expose an inherited key is also `InvalidValue`; the merge key itself has no value at
its text path and returns `NotFound`.

## TOML

```text
Value     = Text(String) | Integer(i64) | Float(f64) | Bool(bool) | Datetime(String)
          | Array(List[Value]) | Table(Map[String, Value])
Position  = { line: u64, column: u64 }
TomlError = Syntax(Position) | DuplicateKey(Position) | Nesting(Position)
          | InvalidValue() | NotFound() | NotTable()
Segment   = Key(String) | Index(u64)
Moment    = OffsetDateTime(time.DateTime, i32) | LocalDateTime(time.DateTime)
          | LocalDate(time.Date) | LocalTime(time.TimeOfDay)
```

`toml.parse` takes a UTF-8 `String` holding a TOML 1.1.0 document and returns its root
`Table`, keys in order of first definition. It accepts every table form (headers,
implicit parents, dotted keys, inline tables, arrays of tables), the four string forms,
integers in decimal, hexadecimal, octal and binary with single underscores between
digits, floats including `inf` and `nan` with either sign, booleans and the four
date-time shapes. Integers are `i64`; a literal outside that range is `Syntax` at its
first digit. Floats round correctly to binary64, overflowing to infinity. Newlines in
multi-line strings decode as LF. A `Datetime` holds the validated source spelling
verbatim: an offset date-time, local date-time, local date or local time, told apart
by shape (a date has `-` at its fifth byte; a date-time adds a time after `T`, `t` or
a space; an offset date-time ends in `Z`, `z` or `±HH:MM`). Months, days (leap years
included), hours, minutes, seconds (up to 60) and offsets are range-checked.
Seconds may be omitted (defaulting to zero); basic strings accept `\e` and
`\xHH` Unicode escapes. Inline tables accept newlines, comments and a trailing comma.

A header defines a table once; a header naming a table first created as another
header's prefix defines it then. Dotted keys extend only tables created by dotted keys
of the same table, so neither a header nor a later section's dotted key may add to
one created elsewhere, and a header may not define a table its dotted keys created.
Inline tables and arrays are complete when they close. `[[a]]` appends an element to
array of tables `a`; a header path through an array of tables names its last
element. Every violation is `DuplicateKey` at the offending key.

`Syntax`, `DuplicateKey` and `Nesting` carry the one-based line and one-based Unicode
scalar column of the offending byte (a tab counts one column; end of input is past the
last scalar). The constant `MAX_NESTING: u64 = 128` counts tables and arrays below the
root, those created by headers and dotted keys included; scalars use no level. The
container that would be level 129 is `Nesting` at its opening bracket, brace or key.

`toml.moment` converts a `Datetime` spelling to `std@1/time` values: an offset
date-time is `OffsetDateTime` with its offset in seconds east of UTC (`Z` is 0),
and the local shapes are `LocalDateTime`, `LocalDate` and `LocalTime`. Fraction
digits beyond nanoseconds truncate. A spelling that is not one of the four shapes,
or a leap second (`time.TimeOfDay` has none), is `InvalidValue`. `toml.datetime`
spells a `Moment` as a `Datetime` value: `T` between date and time, the fraction
without trailing zeros, `Z` for offset 0 and `±HH:MM` otherwise. A year outside
0000 to 9999, or an offset that is not a whole number of minutes within ±23:59, is
`InvalidValue`. `moment` of a value `datetime` returns gives back the same `Moment`.

`toml.stringify` borrows a `Table` and writes a document; any other value is
`NotTable`. Pairs whose value is neither a table nor a non-empty array of tables come
first, then one section per table and per element of such an array (`[[path]]`), each
preceded by a blank line; a table whose members are all such sections has no header of
its own, and an empty table is a bare header. Other arrays and tables nested in arrays
are written inline (`[1, 2]`, `{ a = 1 }`). Keys are bare when they are non-empty and
consist of `A-Za-z0-9_-`. A string or key with a quote or backslash and no apostrophe
or control is a literal string; otherwise a basic string escaping `"`, `\`, `\b`,
`\t`, `\n`, `\f`, `\r` and other controls as `\u00XX`. Floats use the shortest
round-trip digits of the `f64` module, with `.0` appended to whole numbers, and
`nan`, `inf` and `-inf`. A `Datetime` must hold one of the four spellings above, and
no container may exceed level 128; either violation is `InvalidValue`. The output
parses to an equal value, NaN to NaN.

`toml.document` parses like `toml.parse` and keeps the source text; `toml.source`
returns the current text, `toml.root` the root table and `toml.get` the value at a
path, if any. A path's `Key` selects a table member and `Index` an array element,
including an element of an array of tables. Edits never re-render the document: each
splices the text and reparses it, so every byte outside the edited span (comments,
blank lines, ordering, quoting, number spellings) is unchanged, and new lines use the
document's first newline (LF when it has none). A failed edit leaves the document
unchanged.

`toml.set(mut doc, path, value)` with an empty path replaces the document with
`stringify(value)`. Otherwise it validates the value as `stringify` does, then:

- an existing value written inline (a pair's value, an inline-table member, an array
  element) is respelled in place, keeping the key and its comments; a `Table` value
  stays inline there;
- an existing table or array of tables spelled by headers, set to a table or array of
  tables, is replaced by new sections where its first header stood; set to anything
  else it is removed and the value added as below;
- a missing key of a table with a section (the root, a header, an array-of-tables
  element) becomes `key = value` after the section's last pair, with that pair's
  indentation, or right after the header (at the start of the document for the
  root); a member of a table created by dotted keys is added as a dotted key after
  that table's last pair; a member of an implicit table gets a new header placed
  before the table's first sub-table;
- a missing key whose value is a table or array of tables becomes new sections after
  the last line under its parent table (the end of the document for the root);
- a missing member of an inline table is appended before its closing brace;
  `Index(len)` appends to an inline array (on a line of its own when the last
  element's comma ends a line) or adds an `[[path]]` section after the array's last
  element; other missing indices are `NotFound`;
- missing parent tables are created by setting the parent to a one-member table.

A path through a scalar, a `Key` on an array or an `Index` on a table is `NotTable`.
`toml.remove(mut doc, path)` removes the value at `path`, or returns `NotFound`. A
document line takes with it the comment and blank lines above it back to the previous
line and its own trailing comment; a table takes all its headers, sub-tables and pairs;
an inline-table member or array element on its own line takes that line, and
otherwise its comma (the first entry the comma after it, a later entry the comma
before it).

## The formatting protocol

An ordinary `List[u8]` is the UTF-8 text builder. A type formats when std declares how
to append it: `text.append(mut builder, s)` appends a `String`'s bytes, and each scalar
module's `format_into(value, mut builder)` appends that scalar's text. Both only add
valid UTF-8, keep existing content and fail only by allocation exhaustion.
`text.finish_valid(take builder)` returns the builder's text as a `String` and fails `TextInvariant` on invalid UTF-8, which no protocol append produces.
A declared type formats when it names its function with `@format(function)`, a
`fn(value: T, mut out: List[u8]) -> ()` that preserves the builder's uniqueness
([SEM-3](semantics.md#sem-3-strings)); it follows the same append contract.
A scalar's `to_string` is its `format_into` applied to an empty builder, then
`text.finish_valid`; an f-string is the same with one append per piece and hole
([SEM-3](semantics.md#sem-3-strings)). A hole whose options are only fill, alignment
and width is its plain append followed by
`text.pad_from(start: u64, fill: char, align: u64, width: u64, mut out: List[u8]) -> () uses {}`,
or `text.pad_number_from` with the same parameters for an integer or float, where
`start` is the builder's byte length before the append: they pad the text appended since
`start` to `width` estimated display cells, left-aligned and right-aligned by default
respectively. Any other specified hole calls the value's module's
`format_into_spec(value, fill: char, align: u64, width: u64, precision: u64,
flags: u64, kind: u64, mut out: List[u8]) -> () uses {}`; `String` uses
`text.format_into_spec`. These operations preserve the builder's uniqueness.
The parser supplies the options: fill is a `char` (default `' '`), alignment
is 0 default, 1 left, 2 right or 3 center; flags are bit 1 plus, bit 2 alternate,
bit 4 sign-aware zero padding and bit 8 precision present; kind is 0 display or
the ASCII code of `x`, `X`, `b` or `o`. Width and precision are decimal u64 constants
or, written `name$`, the value of a `u64` name in scope.
Ordinary callers must supply these same valid option encodings.

- `{x:9}` sets a minimum width, right-aligned for numbers and left-aligned for
  strings and bool. `<`, `>` and `^` choose left, right and center alignment;
  odd center padding puts the extra cell on the right. `*^9` chooses `*` as fill.
  Width and string precision count estimated display cells over whole extended
  grapheme clusters, using the same calculation as `text.display_width` below.
  On each padding side, repeat as many whole fill characters as fit its cell
  deficit, then append spaces for any remainder. A wide fill never overshoots.
  Fill characters are measured independently, even when adjacent fills or text
  would join them into a cluster (for example, a combining fill counts 1).
- `{x:09}` pads numbers with zeros after their sign and radix prefix, overriding
  alignment and fill. `{x:+}` adds a plus for nonnegative numbers; a minus remains
  on negative values, including negative zero. `-` is the default sign policy.
- `{x:.2}` formats a float with exactly two fractional decimal digits, rounded
  from its exact binary value with ties to even, including carry into the integer
  part. Default float formatting retains its shortest round-trip representation
  and existing fixed/scientific thresholds. Both paths use exact decimal rounding.
  NaN and infinities keep their ordinary spelling; NaN has no sign. Strings and bool are truncated
  to the longest prefix of whole clusters whose estimated width fits the precision
  before padding; integer precision is ignored.
- `{x:x}`, `{x:X}`, `{x:b}` and `{x:o}` format integers in lowercase hex,
  uppercase hex, binary and octal. Signed radix output uses the scalar's two's
  complement bit width. `#` adds `0x`, `0b` or `0o` (also `0x` for uppercase hex).
  Radix formats on other types are SEM-3. String and bool formatting ignores sign,
  alternate and zero flags; alternate display has no effect on numbers.

- `{x:w$}`, `{x:.p$}` and `{x:>w$.p$}` take the width or precision from the `u64` named
  `w` or `p`, exactly as the same constant would.
- A `@format` type's text takes fill, alignment and width as a string's does, counting
  estimated display cells and left-aligned by default; precision, sign, `#`, `0` and radix
  formats on it are SEM-3.

`@noalloc text.display_width(s: String) -> u64 uses {}` returns the sum of the
estimated widths of the first scalar in each Unicode16 default extended grapheme
cluster (`unicode.graphemes`). Following C++23 [format.string.std] (LWG 3780),
a first scalar counts 2 when its pinned Unicode 16.0.0 East_Asian_Width is W or F,
or it is in U+4DC0–4DFF, U+1F300–1F5FF or U+1F900–1F9FF (inclusive).
Every other first scalar counts 1, including isolated combining marks and controls;
the empty string counts 0. The estimate is locale-independent and does not expand
tabs, interpret terminal escapes or promise a particular font's rendering.
For example, `text.display_width("界")` is 2, `text.display_width("e\u{301}")`
is 1, and a family ZWJ emoji beginning with U+1F469 is 2. Precision 1 keeps the
accented cluster whole and omits the CJK cluster entirely. Dynamic `name$` counts
and padded `@format` text use this same estimate.

An empty specification is default formatting. Unknown type characters, malformed
options, overflowing constants and positional (`1$`) or `*` counts are SEM-3.

## Scalars

`bool`, `u8` to `u128`, `i8` to `i128`, `f32` and `f64` each have a module exposing, with
`uses {}`:

- `to_string(value) -> unique String`;
- `format_into(value, mut out: List[u8]) -> ()`, which appends exactly the text `to_string`
  returns and fails only by allocation exhaustion;
- `format_into_spec(value, fill: char, align: u64, width: u64, precision: u64,
  flags: u64, kind: u64, mut out: List[u8]) -> ()`, using the constant option
  encoding above;
- `@noalloc format_inline[n: size](value, mut buffer: Inline[u8, n]) -> Result[u64, scalar.FormatError]`,
  which checks capacity before writing, returns the length of the UTF-8 prefix and leaves
  the whole buffer unchanged on error;
- `@noalloc parse(text: String) -> Result[scalar, scalar.ParseError]`.

`FormatError` is `BufferTooSmall()`; `ParseError` is `Invalid | OutOfRange`. `bool` is
exactly `true` or `false`. Integers use decimal `-?[0-9]+`, with a minus only for signed
types; leading zeros are accepted; a plus sign, whitespace, underscores and radix prefixes
are rejected; unrepresentable values are `OutOfRange`.

Every integer module (`u8`, `u16`, `u32`, `u64`, `u128`, `i8`, `i16`, `i32`,
`i64`, `i128`) additionally declares the following concrete signatures, where
`T` below denotes that module's scalar type, rather than a source generic:

```text
N fn T.checked_add(a: T, b: T) -> Option[T] uses {}
N fn T.saturating_add(a: T, b: T) -> T uses {}
N fn T.wrapping_add(a: T, b: T) -> T uses {}
N fn T.checked_sub(a: T, b: T) -> Option[T] uses {}
N fn T.checked_mul(a: T, b: T) -> Option[T] uses {}
N fn T.saturating_sub(a: T, b: T) -> T uses {}
N fn T.saturating_mul(a: T, b: T) -> T uses {}
N fn T.wrapping_sub(a: T, b: T) -> T uses {}
N fn T.wrapping_mul(a: T, b: T) -> T uses {}
N fn T.parse_radix(text: String, radix: u8) -> Result[T, scalar.ParseError] uses {}
fn T.format_radix(value: T, radix: u8) -> Result[unique String, scalar.ParseError] uses {}
```

Checked operations return `None` exactly when the mathematical result is outside
that type's range. Saturation clamps to its minimum or maximum, according to the
mathematical result; wrapping computes modulo the type's bit width. These are
ordinary source kernels; the add helpers delegate to core `int.*`. Wrapping
multiplication uses a double-width product below 128 bits and three 64-bit limb
products at 128 bits, with no bit-by-bit loop or wider-than-u128 scalar.
Radices are 2 through 36, with case-insensitive ASCII digits on input and lowercase
output. Only signed types accept a leading minus. No plus, whitespace, underscore
or implicit radix prefix is accepted. Invalid radix or grammar yields `Invalid`,
including invalid syntax following overflowing digits; otherwise overflow yields
`OutOfRange`. Decimal `parse` delegates to the same kernel with radix 10.

`char` has a module of its own ([the catalogue](#the-catalogue)): `format_into` appends
the char's UTF-8 encoding, one to four bytes, `to_string` is that text, and `from_ascii`
returns the char of a byte below 0x80 and `None` otherwise; `from_u8` is total, the
char U+0000 to U+00FF whose scalar value is the byte. A char has no `format_inline`,
`format_into_spec` or `parse`; f-string holes of type `char` take only fill, alignment
and width ([SEM-3](semantics.md#sem-3-strings)). Its integer conversions are the core
`char.to_u32` and `char.from_u32`.

Floats format with the shortest decimal digits that round-trip, ties to even. Fixed
notation is used for decimal exponents in [-6, 21), and `d[.ddd]e[-]N` otherwise (the
ECMAScript `Number::toString` thresholds). Whole fixed numbers omit `.0`, negative zero is
`-0`, and the nonfinites are `NaN`, `inf` and `-inf`. Float parsing accepts
`-?[0-9]+(\.[0-9]+)?([eE][-+]?[0-9]+)?` and exactly those nonfinite spellings, and rounds
correctly, ties to even. `OutOfRange` applies exactly when correct IEEE rounding yields
infinity; values above `MAX` that round back to `MAX` succeed. Underflow rounds to a
subnormal or signed zero without error.

The two float modules also declare these ordinary source functions:

```text
N fn f32.floor(value: f32) -> f32 uses {}
N fn f32.ceil(value: f32) -> f32 uses {}
N fn f32.round(value: f32) -> f32 uses {}
N fn f32.sqrt(value: f32) -> f32 uses {}
N fn f64.floor(value: f64) -> f64 uses {}
N fn f64.ceil(value: f64) -> f64 uses {}
N fn f64.round(value: f64) -> f64 uses {}
N fn f64.sqrt(value: f64) -> f64 uses {}
```

Floor rounds toward negative infinity, ceil toward positive infinity and round to
nearest with halfway cases away from zero. All preserve signed zero and infinity,
and canonicalize NaNs under SEM-2. Square root rounds correctly to nearest, ties
to even, handles subnormals, preserves either zero and positive infinity, and
returns canonical NaN for negative nonzero input. The portable baseline uses bit
decomposition and integer arithmetic without a platform math call.

Each float module also declares this math library, shown for `f64`; `f32` declares the
same functions over `f32`:

```text
N fn f64.trunc(value: f64) -> f64 uses {}
N fn f64.fract(value: f64) -> f64 uses {}
N fn f64.abs(value: f64) -> f64 uses {}
N fn f64.copysign(magnitude: f64, sign: f64) -> f64 uses {}
N fn f64.fma(a: f64, b: f64, c: f64) -> f64 uses {}
N fn f64.powi(base: f64, exponent: i32) -> f64 uses {}
N fn f64.exp(value: f64) -> f64 uses {}
N fn f64.exp2(value: f64) -> f64 uses {}
N fn f64.expm1(value: f64) -> f64 uses {}
N fn f64.ln(value: f64) -> f64 uses {}
N fn f64.log2(value: f64) -> f64 uses {}
N fn f64.log10(value: f64) -> f64 uses {}
N fn f64.ln1p(value: f64) -> f64 uses {}
N fn f64.pow(x: f64, y: f64) -> f64 uses {}
N fn f64.cbrt(value: f64) -> f64 uses {}
N fn f64.hypot(x: f64, y: f64) -> f64 uses {}
N fn f64.sin(value: f64) -> f64 uses {}
N fn f64.cos(value: f64) -> f64 uses {}
N fn f64.tan(value: f64) -> f64 uses {}
N fn f64.asin(value: f64) -> f64 uses {}
N fn f64.acos(value: f64) -> f64 uses {}
N fn f64.atan(value: f64) -> f64 uses {}
N fn f64.atan2(y: f64, x: f64) -> f64 uses {}
N fn f64.sinh(value: f64) -> f64 uses {}
N fn f64.cosh(value: f64) -> f64 uses {}
N fn f64.tanh(value: f64) -> f64 uses {}
N fn f64.asinh(value: f64) -> f64 uses {}
N fn f64.acosh(value: f64) -> f64 uses {}
N fn f64.atanh(value: f64) -> f64 uses {}
```

`trunc` rounds toward zero, `fract` is `value - trunc(value)`, `abs` clears the sign,
`copysign` gives `magnitude` the sign of `sign`, and `fma` is `float.mul_add`; these are
exact. `powi` multiplies by repeated squaring, then takes the reciprocal for a negative
exponent, as Rust's `powi` does; its rounding errors compound, so use `pow` when the error
matters. `ln` is the natural logarithm and `ln1p(x)` is `ln(1 + x)`, as `expm1(x)` is
`exp(x) - 1`, both accurate near zero. `atan2(y, x)` is the angle of the point `(x, y)`,
in [-π, π].

The functions are ports of the Rust libm crate (from musl, FreeBSD's msun and
CORE-MATH), the same operations in the same order, so every engine and target computes
the crate's bits. Each result is within the stated number of units in the last place of
the correctly rounded result, the crate's bound tested against MPFR: 0 (correctly
rounded) for `f64.cbrt`, `f64.hypot` and `f32.cbrt`; 1 for exp, exp2, expm1, ln, log2,
log10, ln1p, pow, `f32.hypot`, sin, cos, tan, asin, acos, atan and cosh; 2 for atan2,
sinh, tanh, asinh and atanh; and 4 for acosh. Domain errors give the NaN; poles give
a signed infinity (`ln(0)` is -inf, `atanh(1)` is inf); overflow gives infinity and
underflow zero or a subnormal. `pow` follows IEEE 754 `pow` on special operands:
`pow(x, ±0)` and `pow(1, y)` are 1 even for a NaN operand, and a negative base needs an
integral exponent. `hypot(±inf, NaN)` is inf. `f32` functions may compute in `f64`
internally and round once.

`@noalloc f32.sum(xs: Array[f32]) -> f32` and `f64.sum(xs: Array[f64]) -> f64`, and
`f32.sum_exact` and `f64.sum_exact` with the same types and allocation allowed, implement the
reductions of [the core library](core-library.md#numeric-and-string-details).

## Deque

```text
fn deque.empty[T]() -> unique deque.Deque[T] uses {}
N fn deque.len[T](q: deque.Deque[T]) -> u64 uses {}
fn deque.push_front[T](mut q: unique deque.Deque[T], take value: unique T) -> () uses {q, value}
fn deque.push_back[T](mut q: unique deque.Deque[T], take value: unique T) -> () uses {q, value}
fn deque.pop_front[T](mut q: unique deque.Deque[T]) -> unique Option[T] uses {q}
fn deque.pop_back[T](mut q: unique deque.Deque[T]) -> unique Option[T] uses {q}
```

`Deque[T]` is an opaque source container owning its elements in two unique Lists
of unique elements. Rebalancing reverses each side in place and uses constant
extra space.
Length is O(1); endpoint operations are amortized O(1). When one side is empty,
half the other side moves across, preserving logical order and avoiding repeated
whole-queue reversals when callers alternate removals. Empty pops return `None`.
Push requires a unique input: use `unique value` to copy an aliased value into
a fresh unique owner before pushing. Pop transfers deep uniqueness; leftover items follow ordinary
cleanup,
including resource payloads and failure paths. A fresh empty deque is unique.

## Priority queue

```text
fn priority_queue.empty[T]() -> unique priority_queue.Heap[T] uses {}
N fn priority_queue.len[T](h: priority_queue.Heap[T]) -> u64 uses {}
fn priority_queue.push[T](mut h: unique priority_queue.Heap[T], take value: unique T, by: @noalloc fn(T, T) -> i32 uses {}) -> () uses {h, value}
fn priority_queue.pop[T](mut h: unique priority_queue.Heap[T], by: @noalloc fn(T, T) -> i32 uses {}) -> unique Option[T] uses {h}
```

`Heap[T]` is an opaque source min-heap over a unique List of unique elements.
The module is named `priority_queue` because `heap` is reserved. Callers must use the same ordering
on every operation. Pop transfers the least element under that ordering, or returns
`None` when empty; equal priorities have no stable order. Length is O(1), push and
pop O(log n), with ordinary allocation growth on push. Push requires a unique
input; `unique value` copies an aliased value into a fresh unique owner before
pushing. Push owns its input and
remaining elements are released on cleanup, including comparator failure. Pop and
the heapsort fallback of List sort use sift kernels; reversing the priority-queue
comparator never negates an i32.
A fresh empty heap is unique. This surface provides no peek or indexed update.

## Ordered maps

```text
Bound[K] = Unbounded | Included(K) | Excluded(K)
Direction = Ascending | Descending
fn ordered_map.empty[K: copyable, V: copyable](take by: fn(K, K) -> i32 uses {}) -> ordered_map.OrderedMap[K, V] uses {}
fn ordered_map.len[K: copyable, V: copyable](m: ordered_map.OrderedMap[K, V]) -> u64 uses {}
fn ordered_map.get[K: copyable, V: copyable](m: ordered_map.OrderedMap[K, V], key: K) -> Option[V] in m uses {}
fn ordered_map.contains[K: copyable, V: copyable](m: ordered_map.OrderedMap[K, V], key: K) -> bool uses {}
fn ordered_map.insert[K: copyable, V: copyable](mut m: ordered_map.OrderedMap[K, V], take key: K, take value: V) -> () uses {}
fn ordered_map.remove[K: copyable, V: copyable](mut m: ordered_map.OrderedMap[K, V], key: K) -> Option[V] uses {}
fn ordered_map.first[K: copyable, V: copyable](m: ordered_map.OrderedMap[K, V]) -> Option[(K, V)] in m uses {}
fn ordered_map.last[K: copyable, V: copyable](m: ordered_map.OrderedMap[K, V]) -> Option[(K, V)] in m uses {}
fn ordered_map.fold_range[K: copyable, V: copyable, A](m: ordered_map.OrderedMap[K, V], from: Bound[K], to: Bound[K], direction: Direction, take initial: A, step: fn(take A, K, V) -> (A, bool)) -> A uses {initial, step}
fn ordered_map.valid[K: copyable, V: copyable](m: ordered_map.OrderedMap[K, V]) -> bool uses {}
```

`OrderedMap[K, V]` is a persistent ordered map: an opaque, copyable B-tree of degree 16
(every node but the root holds 15 to 31 entries) whose nodes are ordinary values, holding
the comparator `by` it was made with. Copying a map is O(1) and the copy is a snapshot:
a later `insert` or `remove` on either copy detaches only the O(log n) nodes on its path,
so the other copy never changes, and without a live copy writes update nodes in place.
This is the core `SortedMap`'s sorted sequence with sharing: lookup, insert and remove
take O(log n) comparisons, and insert and remove move O(log n) node entries instead of
O(n). `insert` replaces the value of an equal key; `remove` returns the removed value or
`None()` and leaves every node, and so every copy's sharing, untouched when the key is
absent. `first` and `last` return the least and greatest entries.

`fold_range` visits, in `direction` order, every entry whose key lies between `from` and
`to` (each bound included, excluded or open), passing the accumulator through `step`,
which also returns whether to continue; the walk ends at the first key past the far bound.
Entries are read in place: copying them out is the step's choice, at the step's concrete
types. A caller pages a range by passing `Excluded` of the last key it received.
`valid` checks the tree's invariants (key order across the tree, node sizes, leaves at one
depth and the count) for tests.

## Testing helpers

`test.equal` checks structural equality of `data` values. `test.approx` requires
nonnegative tolerances, accepts exactly equal values, rejects unequal nonfinite values, and
otherwise passes when the difference is within the absolute tolerance or within the
relative tolerance of the larger magnitude. `test.assert_eq` passes on structural equality;
otherwise it reports `assertion failed: actual == expected`, then `  actual: A` and
`expected: E` through `test.log`, rendering each value with `show`, and fails through
`test.fail`. Hirsel has no generic formatting, so the caller names the renderer:
`u64.to_string`, a `@format` type's function, `test.quoted` for strings or
`test.show_list(xs, show)` for lists. `test.expect_error` returns the error of an `Error`,
and destroys an unexpected `Ok` value before failing the test with `test.fail`.

## Property tests

`test.property(t, gen, law, show)` runs 100 cases and `test.property_cases` an explicit
count (zero calls neither callback). A generator `fn(mut test.Source) -> T` draws all of
its randomness from the opaque `Source`, which records every choice; `law` must hold for
every generated value. Case `c` draws fresh choices from a `prng` generator seeded with
`test.seed(t) + c * 0x9e3779b97f4a7c15` (wrapping), so `hirsel test --seed S` replays a
reported run exactly. The first case whose law returns false is shrunk, then its value is
regenerated, reported as `counterexample: V` through `test.log` (rendered by `show`) and
the test fails through `test.check_case` with that case's index, which reports the seed.
A language failure inside `gen` or `law` is not shrunk: it fails the test where it
happens. Every generated value is destroyed before the next case or report.

Shrinking is integrated, after Hypothesis's choice-sequence design and its reference model
minithesis: it edits
the failing case's choice sequence and replays the generator on each candidate, so a
shrunk value is always one the generator can produce and no type needs its own shrinker.
Replay reads the candidate's choices in order, clamping each to the draw's bound; a replay
that draws past the candidate's end, or any case that makes more than 8192 choices, is
discarded. A candidate is accepted when the law still fails and the choices the replay
actually made are simpler: fewer, or as many and lexicographically smaller. The
simplifying passes delete chunks of 8 down to 1 choices (also retrying with the choice
before the chunk lowered by one, which is often a length), zero chunks of 8 down to 2, and
lower each choice to 0 or, unless one less already passes, by binary search. When they no
longer improve, the rearranging passes sort windows of 8 down to 2 choices, swap
out-of-order choices one or two apart and move value from the earlier of such a pair to
the later (all of it, else by binary search unless moving one unit passes). Any
improvement starts another round; shrinking ends when a round improves nothing or 10000
replays are spent. Passes walk from the last choice backwards.

Each draw shrinks toward a documented simplest value:

- `test.choice(mut s, max)` draws from 0 to `max` inclusive, uniformly when fresh, and
  shrinks toward 0; `test.any_u64` is `choice` over every `u64`; `test.bool` is a choice
  of 1 and shrinks toward false.
- `test.range_u64(mut s, low, high)` shrinks toward `low`. `test.range_i64(mut s, low,
  high)` shrinks toward the bound nearest zero, or toward 0 itself when the range contains
  it (a sign choice, then a magnitude). Both fail `Arithmetic` when `low > high`.
- `test.list_of(mut s, max_len, element)` precedes each element with a continue choice
  that is 1 with probability 90% when fresh, so it shrinks toward shorter lists, and
  deleting an element's choices deletes the element. `test.ascii_string(mut s, max_len)`
  builds printable ASCII the same way, each byte shrinking toward `a`.

`test.show_list(xs, show)` renders `[a, b]`; `test.quoted(s)` renders a double-quoted
literal escaping `"`, `\`, line feed and tab, and other C0 controls and DEL as `\u{XX}`.

## Random draws

`random.u64(r)` draws eight bytes with `random.bytes` and reads them little-endian.
`random.below(r, upper)` returns `Error(EmptyRange())` for `upper == 0` before any draw;
otherwise it draws with `random.u64` until the same Lemire acceptance as `prng.below` admits
a value, so every value below `upper` is equally likely, and wraps a provider error as
`Source(e)`. `random.SampleError` is `EmptyRange() | Source(RandomError)`. These functions
add no authority or entropy of their own: their security is the launch provider's.

## Pseudorandom numbers

`prng.Generator` is opaque, noncopyable Xoshiro256++ state (Blackman and Vigna, as in Zig's
`std.Random.Xoshiro256`). `prng.seed(value)` expands the 64-bit seed into the four state
words with four successive SplitMix64 outputs. `prng.u64` returns the next Xoshiro256++
output. `prng.below(g, upper)` returns `Error(EmptyRange())` for `upper == 0` without
advancing; otherwise it draws until Lemire's multiply-shift accepts a draw (rejecting the
low products below `2^64 mod upper`) and returns the high word of the accepted product, so
every value below `upper` is equally likely and modulo reduction is never used.
`prng.bytes(g, count)` returns `count` bytes, each output little-endian, cutting the
last output short. The names match `random`: `u64`, `below` and `bytes` draw the same
shapes from a generator as from the `Random` capability. The generator is a pure deterministic function of its seed: identical
on every engine and target, it holds no authority and is never cryptographically secure.
Programs that need secrets draw from their `Random` capability instead.

## Unicode

Grapheme boundaries and normalization use the
[Unicode 16.0.0](https://www.unicode.org/versions/Unicode16.0.0/) tables and default extended
grapheme clusters, without locale tailoring. `GraphemeView` and `GraphemeCursor` are ordinary
opaque types of `std@1/unicode`; `unicode.graphemes(s)` is a borrowed loop source yielding
each cluster as a `String` related to `s`.

The pure normalization functions borrow a `String` and return `unique String`:

```text
fn unicode.normalize_nfc(s: String) -> unique String uses {}
fn unicode.normalize_nfd(s: String) -> unique String uses {}
fn unicode.normalize_nfkc(s: String) -> unique String uses {}
fn unicode.normalize_nfkd(s: String) -> unique String uses {}
```

NFD recursively applies canonical decomposition and canonical ordering; NFC
also applies canonical composition with composition exclusions and blocking.
NFKD includes compatibility decomposition, and NFKC composes its result.
All four implement algorithmic Hangul decomposition/composition. Normalization
is explicit; equality, ordering and hashing remain bytewise.

Full default Unicode16 string casing and folding are ordinary pure std source:

```text
fn string.to_upper(s: String) -> unique String uses {}
fn string.to_lower(s: String) -> unique String uses {}
fn string.casefold(s: String) -> unique String uses {}
fn string.eq_ignore_case(a: String, b: String) -> bool uses {}
```

Upper/lower combine UnicodeData simple mappings with all unconditional
SpecialCasing mappings, including one-to-many expansions. Lowercase also
applies Final_Sigma: U+03A3 maps to U+03C2 when preceded by a Cased character
and not followed by a Cased character, ignoring Case_Ignorable characters on
both sides; otherwise it maps to U+03C3. Context comes from the original input
slice. These functions have no locale tailoring.

`casefold` applies the full default CaseFolding C+F mappings, excluding simple-only
and Turkic mappings. `eq_ignore_case(a, b)` is `casefold(a) == casefold(b)`.
Neither operation normalizes: `Straße` and `STRASSE` compare equal, while
precomposed `é` and decomposed `e\u{301}` require explicit normalization.
Scalar simple-case helpers and ASCII-only string helpers retain their meanings.

## Bytes, maps, lists and strings

`bytes.replace(src, old, new)` scans left to right, replaces nonoverlapping matches, and
fails SEM-3 for an empty pattern. `bytes.from_inline(xs)` copies a fixed-size
`Inline[u8, n]`, such as a digest, into a fresh `Bytes`; an `Inline` never converts
implicitly. `map.same_entries` compares two maps as sets of entries.
`map.upsert` probes once: for an absent key it copies the borrowed key and stores
`initial()` at the end of insertion order; a bounded arena charges the vacant
entry before calling `initial`. For a present key it calls `change`
with mutable access to the stored value, copying neither key nor value.
`map.update` only calls `change` for a present key and returns whether it found
one. Neither function changes an existing entry's position. Callback mutations
remain visible if the callback fails, under the ordinary mutable-call contract.

`std@1/list` provides `reverse`, `rotate` (left rotation by `amount`), `sort` (in place,
unstable pattern-defeating quicksort with a heapsort fallback, `O(n log n)`, by a
noalloc comparator), `sort_stable` (in place, stable block merge sort, preserving input
order among equal elements, `O(n log n)` time and `O(1)` auxiliary space without
allocation), `binary_search` (the first equal element
of input sorted by the same comparator), `find`, `map`, `filter`, `fold`, `all`, `any`,
`partition`, `concat` and `dedup` (removing consecutive equal elements). `std@1/string`
provides `split` (on a nonempty separator, nonoverlapping, keeping leading, trailing and
empty fields; an empty separator fails SEM-3), `find` (the first match as a `StrPos`),
`split_once(s, separator) -> Option[(String, String)] in s` (the text before and after the
first match, `None()` without one; an empty separator matches at the start, as in Rust),
`split_ascii_whitespace(take s: String) -> List[String]` (on ASCII tab, LF,
form feed, CR and space; omitting empty fields and preserving vertical tab and
non-ASCII whitespace), `starts_with`, `ends_with`, `trim` (ASCII space and `\t` to `\r`, a slice related to the
source), `ascii_lower`, `ascii_upper`, `concat`, `join` and `compare(a, b) -> i32`
(byte-lexicographic, a proper prefix first, as `<` orders `String`; -1, 0 or 1, so
it is a `sort` comparator). `std@1/text` treats a
`List[u8]` as a UTF-8 builder: `append` appends a string's bytes and `finish` validates the
builder into a `String` or returns the bytes.

`split` owns its input. After a borrowed text helper such as `trim`, use `unique`
to make the owned copy: `string.split(unique string.trim(line), " ")`.
`copy` shares the borrowed storage and keeps its locality; it does not satisfy an
owning helper's global input requirement.

## Network addresses and URIs

`std@1/net` parses and formats addresses: `parse_ipv4`, `parse_ipv6`, `parse_ip` and
`parse_socket_addr` return `AddrParseError = Invalid()` on bad input; `ip_to_string` uses RFC
5952 compression for IPv6, and `socket_addr_to_string` brackets IPv6 addresses.
`validate_host` checks a host name, returning `HostError = NameTooLong | InvalidHostName`,
and `ends_in_number` is the numeric-host test of
[CAP-1](capabilities.md#cap-1-authority). Zone and flow identifiers are not represented.

`uri.query_pairs(query)` decodes a query without `?` or a form-urlencoded body to
`Result[unique List[(unique String, unique String)], QueryError]`. It preserves pair
order and repeated keys, ignores empty ampersand fields, splits at the first `=`,
and gives a missing value the empty string. Plus decodes to space. Percent escapes
must contain two hex digits; `InvalidEscape(offset)` identifies the percent byte
in the original input. A literal semicolon is `InvalidSeparator(offset)`. Decoded
strings must be UTF-8 (`InvalidUtf8(component_start)` otherwise). Errors return no
partial pairs. `uri.encode_query(List[(String, String)])` preserves order, writes
spaces as plus and percent-encodes bytes outside the unreserved alphabet with
uppercase hex; it always writes `=` between key and value.

`std@1/uri` parses, formats and resolves RFC 3986 references: `parse`, `to_string`,
`resolve`, `remove_dot_segments`, `percent_encode` and `percent_decode`, with components
kept as `Raw(String) | Encoded(String)` and `UriError = UnexpectedCharacter |
InvalidFormat | InvalidPort | InvalidHostName`. A reference with a syntactically valid scheme
keeps its absolute-parse error; only scheme-less references are parsed as relative, so
`resolve(base, "http://127.1")` is `InvalidHostName`. Hosts apply the numeric-host rule after
percent decoding. Ports contain only ASCII decimal digits and must fit `u16`; empty,
signed and underscored ports are `InvalidPort`. `percent_decode` decodes only `%`
followed by two ASCII hex digits (case-insensitive); malformed or incomplete escapes
stay literal, and plus stays plus. For example `%+f%-0%2F` becomes `%+f%-0/`.
The [prior-art inventory](../uri-prior-art.md) maps the Zig tests and all 42 RFC
3986 §5.4 resolution examples to corpus fixtures and records adaptations.

## Buffered reads

`std@1/bufio` adapts Rust `BufReader`'s refill/consume and delimiter loops. A `Reader`
is opaque buffer state, independent of its source; keep one reader with a source while
unread bytes remain. `ReadError[E] = Source(E) | Limit | UnexpectedEof | InvalidUtf8 |
InvalidRead`. The callback takes explicit mutable source state, appends at most the
requested number of bytes to its output list, and returns the appended count. Zero is
EOF for that operation; a later operation may retry the source. Count/length disagreement
or a count above the requested maximum is `InvalidRead`. The callback must preserve the
existing list prefix.

```text
fn bufio.new(capacity: u64) -> Reader uses {}
fn bufio.buffered(r: Reader) -> u64 uses {}
fn bufio.contents(r: Reader) -> unique Bytes uses {}
fn bufio.consume(mut r: Reader, count: u64) -> () uses {}
fn bufio.read_more[S,E](mut r: Reader, mut source: S, refill: fn(mut S, mut List[u8], u64) -> Result[u64,E], max: u64) -> Result[u64,ReadError[E]]
@preserves_unique(out) fn bufio.read[S,E](mut r: Reader, mut source: S, refill: fn(mut S, mut List[u8], u64) -> Result[u64,E], mut out: List[u8], max: u64) -> Result[u64,ReadError[E]]
@preserves_unique(out) fn bufio.read_exact[S,E](mut r: Reader, mut source: S, refill: fn(mut S, mut List[u8], u64) -> Result[u64,E], mut out: List[u8], count: u64) -> Result[(),ReadError[E]]
@preserves_unique(out) fn bufio.read_until[S,E](mut r: Reader, mut source: S, refill: fn(mut S, mut List[u8], u64) -> Result[u64,E], delimiter: u8, mut out: List[u8], limit: u64) -> Result[u64,ReadError[E]]
fn bufio.read_line[S,E](mut r: Reader, mut source: S, refill: fn(mut S, mut List[u8], u64) -> Result[u64,E], limit: u64) -> Result[Option[unique String],ReadError[E]]
fn bufio.stream_read_more(n: Net, mut s: Stream of n, mut r: Reader, max: u64) -> Result[u64,ReadError[NetError]]
@preserves_unique(out) fn bufio.stream_read(n: Net, mut s: Stream of n, mut r: Reader, mut out: List[u8], max: u64) -> Result[u64,ReadError[NetError]]
@preserves_unique(out) fn bufio.stream_read_exact(n: Net, mut s: Stream of n, mut r: Reader, mut out: List[u8], count: u64) -> Result[(),ReadError[NetError]]
@preserves_unique(out) fn bufio.stream_read_until(n: Net, mut s: Stream of n, mut r: Reader, delimiter: u8, mut out: List[u8], limit: u64) -> Result[u64,ReadError[NetError]]
fn bufio.stream_read_line(n: Net, mut s: Stream of n, mut r: Reader, limit: u64) -> Result[Option[unique String],ReadError[NetError]]
fn bufio.file_read_more(d: Dir, mut f: File of d, mut r: Reader, max: u64) -> Result[u64,ReadError[FileError]]
@preserves_unique(out) fn bufio.file_read(d: Dir, mut f: File of d, mut r: Reader, mut out: List[u8], max: u64) -> Result[u64,ReadError[FileError]]
@preserves_unique(out) fn bufio.file_read_exact(d: Dir, mut f: File of d, mut r: Reader, mut out: List[u8], count: u64) -> Result[(),ReadError[FileError]]
@preserves_unique(out) fn bufio.file_read_until(d: Dir, mut f: File of d, mut r: Reader, delimiter: u8, mut out: List[u8], limit: u64) -> Result[u64,ReadError[FileError]]
fn bufio.file_read_line(d: Dir, mut f: File of d, mut r: Reader, limit: u64) -> Result[Option[unique String],ReadError[FileError]]
fn bufio.stdin_read_more(s: Stdin, mut r: Reader, max: u64) -> Result[u64,ReadError[InputError]]
@preserves_unique(out) fn bufio.stdin_read(s: Stdin, mut r: Reader, mut out: List[u8], max: u64) -> Result[u64,ReadError[InputError]]
@preserves_unique(out) fn bufio.stdin_read_exact(s: Stdin, mut r: Reader, mut out: List[u8], count: u64) -> Result[(),ReadError[InputError]]
@preserves_unique(out) fn bufio.stdin_read_until(s: Stdin, mut r: Reader, delimiter: u8, mut out: List[u8], limit: u64) -> Result[u64,ReadError[InputError]]
fn bufio.stdin_read_line(s: Stdin, mut r: Reader, limit: u64) -> Result[Option[unique String],ReadError[InputError]]
```

`new(0)` uses capacity one. Capacity bounds each refill, not accumulated unread data.
`read_more` compacts consumed bytes and appends a single bounded refill even if bytes
remain; bounded protocol parsers must also bound their accumulated input. `buffered`
reports unread bytes, `contents` returns an owned snapshot of them without reading, and
`consume` advances by the smaller of its count and the unread length.

`read` appends at most `max` bytes, using unread bytes first, and returns zero at EOF.
`read_exact` appends exactly `count` bytes or returns `UnexpectedEof`, preserving partial
output. Zero `max` or `count` succeeds without calling the source. `read_until` appends
through the delimiter, including it, or through EOF, and returns the appended count.
Its limit bounds newly appended bytes; `Limit` preserves partial output and leaves the
next byte unread. A zero limit returns `Limit` without reading. `read_line` applies that
loop to LF and decodes owned UTF-8, preserving the LF; EOF before any bytes yields
`None()`, as `input.read_line` does, so `while let Some(line) = ..?` reads every line.
Invalid UTF-8 returns `InvalidUtf8` after consuming those bytes. All reads propagate source
errors as `Source`; operations taking an output list retain already appended bytes. The Stream adapters use
`core@1/net.read` with the same rules and preserve the stream's Net brand.
The File adapters mirror them using `core@1/file.read`, preserving `File of d`.
The Stdin adapters mirror them using `core@1/stdin.read`. A Stream or File is mutable
source state, but `Stdin` is a borrowed, copyable capability token, as `Stdout` is for
writes, so `stdin.read` does not fit the refill's `mut S` parameter: read stdin through
these adapters rather than a hand-written refill.
Discard buffered state before seeking or otherwise reading the underlying File directly.

A line read over an existing branded Stream needs only persistent buffer state:

```hirsel
fn line(n: Net, mut stream: Stream of n) -> Result[Option[String], bufio.ReadError[NetError]] {
  let mut reader = bufio.new(4096)
  bufio.stream_read_line(n, mut stream, mut reader, 8192)
}
```

## HTTP wire format

`std@1/http_wire` holds noalloc, `uses {}` parsers that return byte offsets into a borrowed
buffer rather than slices, so no result carries a relation:

```text
RawHeader   = { name_start: u64, name_end: u64, value_start: u64, value_end: u64 }
ParseStatus = Complete(u64) | Partial
StatusLine  = { version: u8, code: u16, reason_start: u64, reason_end: u64 }
HeadError   = HeaderName | HeaderValue | NewLine | Status | Token | TooManyHeaders
            | Version | ContentLength | TransferEncoding | Continuation
ChunkSize   = Complete(u64, u64) | Partial         // (bytes consumed, chunk size)
ChunkError  = InvalidChunkSize()
Framing     = { content_length: Option[u64], chunked: bool, keep_alive: bool }
```

`parse_response` and `parse_headers` follow httparse's strict response grammar: no lenient
whitespace and no obsolete line folding (`Continuation`). They reset `count` to the
initialized header prefix on every outcome, `Partial` and errors included, and their
`Complete` offset is absolute in `buf`. `parse_chunk_size` follows httparse (at most 16 hex
digits, `;` starts an extension, CRLF required) and returns a consumed count relative to
`start`. `framing` interprets `connection`, `content-length` and `transfer-encoding`
case-insensitively; it rejects a message with both `transfer-encoding` and
`content-length`, unequal repeated `content-length` values and transfer codings other than
`chunked`; a `content-length` is `1*DIGIT` with overflow checks. `header_iterator` starts
after the first line, and `next_header` returns trimmed offsets, sets `is_trailer` when it
crosses the empty line into trailers, and stops on an empty name or an incomplete line.
`find_header` returns the first case-insensitive name match among parsed headers.

## HTTP client

`std@1/http` is an HTTP client over the core network operations, with HTTP/1.1
and explicit prior-knowledge HTTP/2 transports:

```text
Method       = Get | Head | Post | Put | Delete | Patch | Options
Version      = Http10 | Http11 | Http2
Header       = { name: String, value: String }
Config       = { max_redirects: u32, connect_timeout: Option[Duration],
                 read_timeout: Option[Duration], write_timeout: Option[Duration],
                 max_head_bytes: u64, max_body_bytes: u64, idle_timeout: Option[Duration],
                 max_idle_connections: u64, max_idle_connections_per_host: u64,
                 max_headers: u64, user_agent: String,
                 roots: TlsRoots, status_as_error: bool, decompress: bool,
                 unix_socket: Option[Path], http2_prior_knowledge: bool }
ResponseHead = { version: Version, status: u16, reason: String, headers: List[Header],
                 content_length: Option[u64], chunked: bool, keep_alive: bool,
                 location: Option[String], content_type: Option[String] }
Response     = { status: u16, headers: List[Header], body: Bytes, url: String }
HttpError    = Net(NetError) | Url(uri.UriError) | Malformed(http_wire.HeadError)
             | Chunk | HeadTooLarge | BodyTooLarge | Truncated | TooManyRedirects
             | RedirectRequiresResend | RedirectLocationInvalid | UnsupportedScheme
             | UnsupportedTransferEncoding | Status(u16) | OutOfOrder | Closed | RequestTimeout | Clock(ClockError) | Json(json.JsonError) | InvalidJsonUtf8
             | Decode(flate.FlateError) | UnsupportedContentEncoding

pub opaque resource type Request: noalloc_drop uses {Net}

fn default_config() -> Config uses {}
fn request(n: Net, config: Config, method: Method, url: String, headers: List[Header]) -> Result[Request, HttpError]
fn send_bodiless(mut r: Request) -> Result[(), HttpError]
fn send_body(mut r: Request, body: Bytes) -> Result[(), HttpError]
fn write_request_body(mut r: Request, data: Bytes) -> Result[(), HttpError]
fn end_request_body(mut r: Request, request_trailers: List[Header]) -> Result[(), HttpError]
fn receive_head(mut r: Request) -> Result[ResponseHead, HttpError]
@preserves_unique(out) fn read_body(mut r: Request, mut out: List[u8], max: u64) -> Result[u64, HttpError]
fn read_all(mut r: Request, limit: u64) -> Result[unique Bytes, HttpError]
fn trailers(r: Request) -> List[Header]
fn header(headers: List[Header], name: String) -> Option[String] uses {}
fn decode_content(headers: List[Header], body: Bytes, limit: u64) -> Result[unique Bytes, HttpError] uses {}
fn fetch(n: Net, config: Config, method: Method, url: String, headers: List[Header], body: Option[Bytes]) -> Result[Response, HttpError]
```

`default_config()` is 3 redirects, no timeouts, 64 KiB of head, 100 headers, user agent
`hirsel-std/1`, `WebPki()` roots, `status_as_error` true, `decompress` false and no `unix_socket`. A `Request` holds its `Net`, a
`Stream` branded by it, the parsed URL, the redirect budget, an input buffer and the
response state, so its operations take only `mut r` and its authority is `{Net}`; dropping
it drops the stream.

- `request` parses the URL, maps `http` and `https` to ports 80 and 443 (other schemes are
  `UnsupportedScheme()`), connects with the connect timeout, installs the read and write
  timeouts, and for `https` upgrades with `tls.connect` using the URL's host and the
  configured roots, so the timeouts also bound the handshake. Header names and values
  containing CR or LF, and names containing `:`, are `Malformed(...)`: this is the
  header-injection boundary for agent-supplied strings.
- With `unix_socket` set to `Some(path)`, as with curl's `--unix-socket`, every connection,
  including each redirect's, is `net.unix_connect(n, path)` in place of the TCP connect, so
  the `Net` needs `(path, 0, UnixConnect())` and no TCP rule. The URL still supplies the
  scheme, the `host` header, the request target and, for `https`, the `tls.connect` server
  name; its port is unused. `net.unix_connect` takes no timeout, so `connect_timeout` does
  not apply; the read and write timeouts do. A `Client` pools these connections by the
  URL's origin as usual.
- `send_bodiless` and `send_body` write the request head (`METHOD target HTTP/1.1`, `host`,
  `user-agent`, `connection: close`, the caller's headers, and `content-length` for a
  body) and the body. `write_request_body` and `end_request_body` stream a `transfer-encoding: chunked`
  body with trailers. Calls out of order return `OutOfOrder()`.
- `receive_head` reads at most `max_head_bytes` (`HeadTooLarge()` beyond), parses the head,
  skips `100 Continue` and follows redirects. A response to `HEAD`, a 1xx, 204 or 304 has no
  body. Header values must be valid UTF-8 (`Malformed(HeaderValue())` otherwise); an
  invalid UTF-8 reason phrase becomes an empty `reason` without failing.
- `read_body` appends at most `max` body bytes, framed by Content-Length, chunked transfer
  or end of stream; 0 means the body is complete. End of stream before the declared length
  or the final chunk is `Truncated()`. `read_all` collects the body up to `limit`, returning
  `BodyTooLarge()` beyond it.
- `fetch` is the one-shot form: method, URL, headers and an optional body, returning the
  whole response with a body limit of 10 MiB. With `status_as_error`, a final 4xx or 5xx
  status is `Status(code)`.

Content-Length and chunked framing are supported in both directions, with the rejections of
`http_wire.framing`. No `accept-encoding` is sent unless `decompress` is set (below). **Redirects** happen only for bodiless
requests or after a 303; a 303, and a 301 or 302 after a `POST`, become a bodiless `GET`;
a 307 or 308 after a body has been sent is `RedirectRequiresResend()`; more than
`max_redirects` is `TooManyRedirects()`. The `Location` is resolved against the current URL
with `uri.resolve`. `authorization`, `cookie` and `proxy-authorization` are kept only within
the exact canonical origin (scheme, host and port). A redirect target goes through the same
`Net`, so a target outside the grant fails with `Net(NotGranted())`, and an https-to-http
downgrade fails under a `ConnectTls` rule.

`Client` is an opaque resource with authority `{Net, MonotonicClock}`. Construct it with
`client(n, clock: Option[MonotonicClock], config) -> Result[Client, HttpError]` and use
`client_fetch(mut c, method, url, headers, body) -> Result[Response, HttpError]` for
persistent requests. `close_idle(mut c)` drops every idle stream and leaves the client
usable. Dropping the client drops its pool. The exclusive mutable borrow permits one
exchange at a time; concurrent workers each own a client.

Idle connections are keyed by canonical `(scheme, host, port)` (including default ports),
and reused only after a complete, framed response whose connection tokens permit
persistence. All Connection fields contribute tokens; `close` wins. EOF-framed responses,
protocol upgrades, extra unread bytes, failed reads/writes, malformed responses and bodies
above `max_body_bytes` do not return their connection. A stale connection is dropped on
failure, with no automatic retry for any method: the caller decides whether resending is
safe. A fully consumed status-error response may return its connection before `Status`.
One-shot `request` and `fetch` continue sending `connection: close`.

`max_idle_connections` defaults to 16 for the entire client and evicts the oldest idle
connection when full; zero disables pooling. `max_idle_connections_per_host` defaults to 2,
must be positive and bounds idle connections retained per origin. These limits do
not impose a dialing budget or count an active exchange. Sequential borrowing
normally needs only one connection per origin.
`idle_timeout` defaults to 30 seconds and requires `Some(clock)` at construction;
`None()` disables expiry and permits `None()` for the clock. Expired connections are
dropped at checkout, before reuse. Invalid timeout values or a missing required clock
return `Net(InvalidInput())`. These are ordinary std resources and core calls.

For a whole-request deadline, enclose the call in the landed core scope API:

```hirsel
fn bounded_fetch(
  tasks: Tasks,
  deadline: Deadline,
  take client: http.Client,
  url: String
) -> Bounded[(http.Client, Result[http.Response, http.HttpError])] {
  task.scope_until(
    tasks,
    deadline,
    fn(s: Scope) -> (http.Client, Result[http.Response, http.HttpError]) {
      let mut current = take client
      let outcome = http.client_fetch(mut current, Get(), url, [], None())
      (take current, take outcome)
    }
  )
}
```

The once scope body takes the client and returns it with the outcome. Match
`Finished((client, outcome))` or `Expired((client, outcome))` to recover both and
rebind the client as mutable for the next call. This uses owned captures; the default
borrowed capture does not permit mutation of an outer client.

The enclosing scope bounds connection setup, TLS, redirects, writes and the entire
response read with one absolute deadline. `Finished(value)` means the scope completed;
`Expired(value)` retains its body's result after deadline cancellation. A blocked
exchange returns `Net(Canceled(DeadlineExceeded()))` and drops its checked-out stream,
so later calls can use only other healthy idle connections or fresh ones. No per-call timeout argument
or extra polling loop is involved. The same pattern applies to one-shot `request`/`fetch`
and incremental response reads. Config's existing connect/read/write timeouts remain
relative network-provider bounds and return `TimedOut`, independently of a scope deadline.


`Body = { bytes: Bytes, content_type: String }` pairs an encoded body with its media type.
`form_body(pairs: List[(String, String)]) -> Body` uses `uri.encode_query`, preserving
pair order, repeated keys and empty values, encoding spaces as `+` and literal `+` as
`%2B`; its media type is `application/x-www-form-urlencoded`.
`json_body(value: json.Value) -> Result[Body, HttpError]` uses `json.stringify_into` and selects
`application/json`. Invalid JSON number representations return `Json(InvalidNumber())`.

`client_send(mut c, method, url, headers, body: Body) -> Result[Response, HttpError]`
calls `client_fetch` with the encoded bytes, replacing all caller Content-Type fields
with exactly one field carrying the body's media type. Other headers keep their order.
The transport still owns Content-Length and Transfer-Encoding.
`response_json(response: Response) -> Result[json.Value, HttpError]` validates the body
as UTF-8 and parses it with std JSON. Invalid UTF-8 is `InvalidJsonUtf8()`; parse and
serialization errors are `Json(json.JsonError)`. Decoding does not require a response
Content-Type and leaves the original response available through the borrowed argument.

**Content codings** follow Go's Transport compression, as an explicit opt-in.
With `decompress` true and no caller `accept-encoding` header, `fetch` and
`client_fetch` send `accept-encoding: gzip, deflate` and pass a nonempty response
body through `decode_content`, bounded by the same limit as the raw body (10 MiB
for `fetch`, `max_body_bytes` for a client). The decoded `Response` omits
`content-encoding` and `content-length`. A caller that sends its own
`accept-encoding` receives the body as sent, and an empty body (HEAD, 204, 304,
Content-Length 0) is returned unchanged. Decoding follows status handling, and a
connection whose exchange completed returns to the pool even when decoding fails.
The streaming `request`, `receive_head` and `read_body` never decode.

`decode_content` removes the codings of every `content-encoding` field in reverse
of their listed order (RFC 9110 section 8.4): `gzip` and `x-gzip` with
`flate.gzip_decode`, `deflate` with `flate.zlib_decode`, and `identity` and empty
list elements not at all; names are case-insensitive. Each decoded stage and the
result hold at most `limit` bytes (`BodyTooLarge()` beyond); other decoding failures
are `Decode(e)`; any other coding is `UnsupportedContentEncoding()`, which a server
decoding a request body answers with 415.

## Input

`input.read_all(s: Stdin) -> Result[unique Bytes, InputError]` reads to end of input.
`input.read_line(s: Stdin) -> Result[Option[unique String], InputError]` reads through
one newline, retaining it (and a preceding carriage return). It returns `None()`
only for EOF before any bytes, and `InvalidUtf8()` for an invalid complete line.
Both are ordinary source loops over `core@1/stdin.read`; they grant no authority
and share the passed stream's cursor. A final unterminated line is `Some`.

Exact environment-name authority is available in core through `env.get(Env, String)`.
The run configuration's Env object is the concrete exact-name facet: only its keys
are readable, with null representing a granted but unset name.

## Planned, not in std@1

The items in this section are designed but not yet declared by `std@1`: a program that
names them does not resolve. They are recorded here so a later `std` version can add them
under this contract; until then only the CAP-5 corpus package fixtures
(`packages/fixtures/seal_box@1`, `seal_box@2`) exercise the sealing they rely on.

### Capability facets

The planned read-only facets are sealed capabilities
([CAP-5](capabilities.md#cap-5-capability-sealing)).

`ReadDir` is an opaque global capability with `plain`, `sendable` and `noalloc_drop`
promises and a private `Dir` representation and ceiling. Its operations are
`@noalloc fn read_dir.of(d: Dir) -> ReadDir`,
`fn read_dir.narrow(r: ReadDir, p: Path) -> Result[ReadDir, FileError]`,
`fn read_dir.read(r: ReadDir, p: Path) -> Result[Bytes, FileError] in r`,
`fn read_dir.size(r: ReadDir, p: Path) -> Result[u64, FileError]` and
`fn read_dir.walk(r: ReadDir) -> Result[ReadWalk, FileError]`. `ReadWalk` is an opaque
global sealed resource, not plain, with public authority `{ReadWalk}`, private
`Dir`-bearing iterator state and an explicit ceiling covering that state and its cleanup;
its operation is `fn read_walk.next(mut walk: ReadWalk) -> Option[Result[Path, FileError]]`.
The read-only interface exports no unwrapping, mutation, raw-grant callback,
authority-bearing read result or effectful cleanup.

The optional sealed wrapper `EnvKey` remains planned; the exact-name grant
discipline itself is implemented by core `env.get`. `EnvKey` is an opaque global capability holding `Env` and one validated, immutable `String`
key, with public authority `{EnvKey}`, a private `Env` ceiling and truthful `copyable` and
`noalloc_drop` promises; its `String` makes it counted, not plain. Its operations are
`fn env_key.of(env: Env, take key: String) -> Result[EnvKey, EnvError]` and
`fn env_key.get(key: EnvKey) -> Result[Option[String], EnvError]`. `get` has no argument
that can replace the stored key. Construction validates that the key is granted; a granted
but unset key stays distinct from a denied one. Facets receive actual grants and cannot
reconstruct authority from a run record.

### Stores

`std@1/store`, an embedded transactional store written in Hirsel over a `Dir` grant, with
typed tables, schema history and migrations, is specified in [storage](storage.md).

### HTTP/2 through the HTTP API

`Version` includes `Http2`. `Config.http2_prior_knowledge` defaults to false.
Setting it true explicitly selects cleartext h2c, with the RFC 9113 connection preface;
HTTP/1 Upgrade is never attempted. The client applies this option only to `http` URLs.
`protocol(stream)` and `request_protocol(request)` expose the selected version, while
parsed request/response heads carry their actual version. TLS negotiation requires
`tls.connect_with` and `tls.negotiated_protocol` from the ALPN provider integration;
until those APIs land, the default HTTPS path remains HTTP/1.1.

The existing client operations, `Response`, `Body`, redirects, body bounds and content
decoding work over h2c. Header names are lowercase on the wire; method, scheme, target
and authority map to the HTTP/2 pseudo-fields. Server requests expose `:authority`
through the ordinary `host` header lookup. `receive_head` skips informational heads.
`write_request_body` and `write_response_body` flush DATA prefixes under stream and
connection windows, reading WINDOW_UPDATE when blocked. End operations emit END_STREAM
or a final trailer block. `read_body` returns available DATA prefixes and returns flow
credit only after releasing a received fragment; trailers retain the same API. These
provider reads and writes record/replay through Net exactly as HTTP/1 operations do.

## HTTP server

The server shares the client's wire parser and error type:

```text
RequestLine = { method_start: u64, method_end: u64, target_start: u64, target_end: u64, version: u8 }
N fn http_wire.parse_request[k: size](buf: Bytes, mut line: RequestLine, mut headers: Inline[RawHeader, k], mut count: u64) -> Result[ParseStatus, HeadError] uses {}
ServerRequest = { method: String, target: String, version: Version, headers: List[Header], body: Bytes, trailers: List[Header], keep_alive: bool }
fn http.parse_server_request(buf: Bytes, config: Config) -> Result[Option[(ServerRequest, u64)], HttpError] uses {}
fn http.server_connection(tasks: Tasks, n: Net, take stream: Stream of n, config: Config) -> ServerConnection
fn http.serve_tls(tasks: Tasks, n: Net, take stream: Stream of n, server: TlsServer, config: Config) -> Result[ServerConnection, HttpError]
fn http.next_stream(mut c: ServerConnection, clock: Option[MonotonicClock]) -> Result[Option[(sendable ServerStream, unique ServerRequest)], HttpError]
fn http.protocol(s: ServerStream) -> Version uses {}
fn http.request_protocol(r: Request) -> Version uses {}
fn http.respond(mut s: ServerStream, status: u16, headers: List[Header], body: Bytes, close: bool) -> Result[(), HttpError]
```

`parse_request` follows httparse's strict request-line delimiters, accepts HTTP/1.0 and
HTTP/1.1, and reuses `parse_headers`. Its initialized-header count and absolute consumed
head offset follow `parse_response`. `parse_server_request` returns an owned request and
the consumed head-plus-body byte count, or `None` for incomplete input. Both it and
`next_stream` enforce `Config.max_headers` (100 by default), `max_head_bytes` and
`max_body_bytes`, and read a Content-Length or chunked body (zero bytes when neither
framing is present). The 16-entry parser scratch buffer is a batch size, never a
message limit. Header counts and framing checks span every batch. Trailer blocks
obey the same header-count and head-byte limits. Malformed heads, conflicting Content-Length and Transfer-Encoding, and
unsupported transfer codings fail. Bytes after the declared body are excluded from the request.

Chunked requests reuse `http_wire.parse_chunk_size`, including chunk extensions and
split CRLF. The body limit applies to the decoded bytes; advertised chunks exceeding
the remaining bound fail before their payload is read. Each size line and the trailer
block obey the head byte limit. `ServerRequest.trailers` contains the decoded trailer
headers, separate from the original headers; Content-Length, Transfer-Encoding, Host
and Connection are forbidden there. Truncated framing or payload is `Truncated`.
The pure parser and socket receiver share one incremental body decoder. The receiver
consumes completed framing and payload as it progresses, retaining decoded data and
only the unread wire suffix for pipelined requests.

`ServerConnection` owns the transport between requests. `next_stream` yields an opaque,
owned `ServerStream` and the decoded `ServerRequest`, or `None` at clean EOF. Responses
are sent only on that stream. It is sendable, so a handler task may take it together
with its request. The constructor takes the run's `Tasks` for the channel that returns
completed transports; it creates no task. While a stream remains active, another
`next_stream` on the connection returns `OutOfOrder`. Completing `respond` or
`end_response_body` automatically returns the transport; it needs no explicit finish
operation. Every response operation on an already completed stream is `OutOfOrder`.
Dropping an unfinished stream closes the transport and makes `next_stream` return `None`.
This includes HTTP/2 in this single-owner implementation; the split dispatcher will
replace HTTP/2 connection closure with `RST_STREAM` for an abandoned exchange.

Config uses `max_head_bytes`, `max_body_bytes` (10 MiB by default), `read_timeout`,
`write_timeout`, and `idle_timeout` (30 seconds by default, `None` for unbounded).
HTTP/1 pipelined responses remain in request order. HTTP/2 frames for other streams
are retained while the active exchange completes, bounded by `max_body_bytes` including
queued event metadata. Concurrent dispatch requires the split transport provider.
The single-owner adapter uses the granted Tasks provider only for transport return
channels and state flags; scheduler choice remains run configuration policy.

Persistence follows RFC 9112 section 9: HTTP/1.1 persists by default; HTTP/1.0 requires
an explicit keep-alive token. Every Connection field contributes comma-separated,
case-insensitive tokens; close takes precedence over keep-alive. `respond` validates
status and headers and owns Content-Length, Transfer-Encoding and Connection framing.
For HEAD, `respond` emits the Content-Length of the supplied body but no payload.
For 204 and 304, it emits neither Content-Length nor payload, even when the supplied
body is nonempty. Complete and streaming responses share this body suppression rule.
Statuses below 200 remain unsupported and return `Malformed(Status())` before writing.
Its `close` argument lets the handler end a connection even when the request permits
reuse. It emits Connection: keep-alive or close accordingly. Parse failures are answered by the endpoint before application handling, always closing
HTTP/1: malformed or truncated requests use 400 with `Bad Request`, oversized bodies
413 with `Payload Too Large`, oversized heads 431 with `Request Header Fields Too Large`,
and invalid versions 505 with `HTTP Version Not Supported`. The returned `HttpError`
still identifies the cause. A failure writing the rejection returns its network error.
HTTP/2 uses its protocol stream or connection errors rather than an HTTP/1 reply. Any failed response ends reuse. A closed
connection, clean EOF between requests, or an expired keep-alive idle wait returns
`None`; EOF within a request is `Truncated`. Dropping the resource closes the socket.
For a server, `Config.read_timeout` bounds the whole request, including the head,
Content-Length or chunked payload, and trailers. `next_stream` takes an explicit
`Some(clock)` for a finite bound; `None` needs no clock authority for an unbounded
request. A finite bound without a clock is `Net(InvalidInput())` before any read.
Clock failures are `Clock(ClockError)`; a deadline beyond the clock range is
`Clock(OutOfRange())`. A reused connection's idle wait is separately bounded by
`idle_timeout`; its request budget starts when the first bytes arrive.

One absolute deadline is sampled per request. Each socket read waits with the
remaining interval using the existing network readiness/deadline waits; successful
short reads never restart the budget, and there is no polling loop. On expiry the
receiver sends 408 with Connection: close and returns `RequestTimeout`; a failure
writing that response propagates its network error. The connection cannot be
reused or receive a second response. Dropping its resource closes the socket.

Servers answer malformed requests before application handling. The
caller owns the listen/accept loop, routing and concurrency across connections.
`ServerRequest.body` keeps its content codings; `decode_content` removes them.

`serve_tls` serves HTTPS on an accepted stream: it sets the stream's read and write
timeouts from `read_timeout` and `write_timeout` (unbounded when `None`), so they bound
each socket wait of the server handshake, performs `tls.accept` with `server`, and returns
`server_connection` over the secured stream; every later receive sets its own read
timeout as above. A handshake or timeout failure is `Net(error)` and closes the stream.
The connection then behaves exactly as a plaintext one; the handshake selects ALPN
`http/1.1` only, replacing `server.alpn`, so a client offering ALPN without it fails the
handshake. A server that reads the client's certificate chain performs
`tls.accept` and `tls.peer` itself and passes the secured stream to `server_connection`.

#### Streaming HTTP responses and EventSource framing

```text
fn http.respond_head(mut s: ServerStream, status: u16, headers: List[Header]) -> Result[(), HttpError]
fn http.write_response_body(mut s: ServerStream, bytes: Bytes) -> Result[(), HttpError]
fn http.end_response_body(mut s: ServerStream) -> Result[(), HttpError]
SseError = EventLineBreak | IdLineBreak | IdNul
fn http.sse_event(event: Option[String], data: List[String], id: Option[String], retry: Option[u64]) -> Result[unique Bytes, SseError] uses {}
```

`respond_head` starts a response without a known length, after `next_stream`. It accepts final statuses 200–599,
validates headers, and reserves Content-Length, Transfer-Encoding and Connection
just as `respond` does. It emits the validated request's HTTP version and no
Content-Length. Malformed heads never yield a response handle; the endpoint sends its fixed rejection.
Neither version nor HEAD semantics carry over from a previous request.
Following [RFC 9112 sections 6.3 and 7.1](https://www.rfc-editor.org/rfc/rfc9112.html#section-6.3),
HTTP/1.1 bodies use Transfer-Encoding: chunked; HTTP/1.0 bodies use close delimiting,
with Connection: close even when the request asked for keep-alive. HEAD, 204 and 304
responses have no body or Transfer-Encoding, and `write_response_body` elides their data.

`write_response_body` sends each nonempty fragment as one hexadecimal-sized chunk on HTTP/1.1,
or directly on HTTP/1.0. Writes go directly through the stream's `write_all`, so the
head and every body fragment are flushed before returning, without accumulating the
whole body. An empty fragment sends nothing and never terminates the body.
`end_response_body` sends the terminating zero chunk with empty trailers for a chunked response.
It restores request reception only when persistence is permitted. Otherwise it shuts
down the stream, including for HTTP/1.0 close-delimited responses. The configured
write timeout applies to head, fragment and completion writes. Receiving a request or
starting another response while a body is open, or writing/ending without an open
body, returns `OutOfOrder`. Any failed write prevents reuse. Dropping a connection
mid-body closes its owned stream without synthesizing a terminating chunk.

Clients already stream through `request`, `send_bodiless`/`send_body`, `receive_head`
and repeated `read_body(mut r, mut out, max)`: a positive read returns up to `max`
available body bytes without waiting for the whole response, and zero means the
framed body is complete. Dropping an incomplete request closes its stream.

`sse_event` produces a UTF-8 block for the
[WHATWG EventSource format](https://html.spec.whatwg.org/multipage/server-sent-events.html#event-stream-interpretation).
Each data item emits a `data: ` field; embedded CRLF, CR or LF splits it into further
fields, preserving empty lines. Optional event, id and retry fields are omitted for
`None`; `Some("")` emits an empty event or id field (an empty id resets Last-Event-ID).
Retry is decimal milliseconds, including zero. A blank line completes the block;
without any data fields it updates fields without dispatching a message.
Event names containing CR or LF return `EventLineBreak`; ids containing CR or LF
return `IdLineBreak`, and ids containing NUL return `IdNul`. No field is silently
removed and no partial bytes escape on error. Data may contain NUL. Pass the bytes
to `write_response_body` with a `Content-Type: text/event-stream` response header. Application
protocols such as Datastar stay in their own packages.

## HPACK and explicit HTTP/2 connections

`std@1/hpack` implements RFC 7541 over opaque byte sequences. A `Field` holds
`name: Bytes`, `value: Bytes` and `sensitive: bool`. Use one `Encoder` and one
`Decoder` per connection direction, initially `encoder(4096)` and
`decoder(4096)`. `encode(mut encoder, fields)` emits a complete header block;
`decode(mut decoder, block, limit)` decodes a complete block. The decoder
bounds the sum of uncompressed name/value lengths plus 32 octets per field,
including indexed fields, before emitting a field. Huffman expansion observes
the same bound. A decode error permanently invalidates that context: reuse
returns `Failed`. The caller closes the connection after a compression error.
`set_capacity` applies the peer's table-size setting and emits the smallest
intermediate size and final size at the next block. `set_allowed_capacity`
changes the decoder's locally advertised bound; a reduction requires a peer
size update at the start of its next block. Sensitive fields always encode as
never-indexed literals, even for exact table matches. `huffman_encode` and
`huffman_decode` expose the static code, rejecting EOS symbols, invalid padding,
overlong padding, truncation and expansion beyond the supplied limit.

`std@1/http2_wire` provides RFC 9113 frame validation and serialization.
`Frame` holds `kind: u8`, `flags: u8`, `stream: u32`, `payload: Bytes`.
`parse(bytes, offset, max_size)` returns `Ok(None())` for incomplete input or
`Ok(Some((frame, next_offset)))`; its frame borrows the input. It rejects an
oversized payload from the nine-octet header, before awaiting more bytes,
and ignores the reserved stream bit. `validate` checks a complete frame;
`encode(frame, max_size)` also rejects a reserved bit supplied by the caller.
`fragment(frame)` borrows DATA or HPACK octets without padding or priority
fields. Unknown types and flag bits remain available to the connection layer.
Errors distinguish `ConnectionError(Code)` and `StreamError(stream, Code)`.

`std@1/http2` supplies an explicit HTTP/2 endpoint. `session(Client() | Server(),
header_limit)` owns compression state, multiplexed stream state and separate
connection/stream flow-control windows. Its client output begins with the
prior-knowledge preface and SETTINGS disabling push. The server expects its
caller to verify that preface before passing frames. Both require the peer's
first frame to be non-ACK SETTINGS. `drain_frames(mut session)` returns the
pending preface and frames in wire order without coalescing payloads. Write the
preface before the encoded frames. `output(mut session)` instead coalesces
pending wire bytes into one buffer. `receive(mut session, frame)` produces `Headers`, `Data`, `Reset`,
`Goaway`, `PingAck`, `Settings` or `Idle` events, queuing protocol acknowledgements
and errors. A connection error queues GOAWAY and permanently fails the endpoint;
a stream error queues RST_STREAM. Push promises are connection errors. Header
blocks must be contiguous HEADERS/CONTINUATION frames for the same stream;
compressed blocks and decoded header lists each obey `header_limit`.

`open` allocates an odd, increasing client stream ID without awaiting earlier
responses. `headers` sends response headers or trailers. `send_data` accepts
only the prefix allowed by both send windows and the peer's frame-size setting;
zero means await WINDOW_UPDATE. Its END_STREAM bit is sent only when the
complete supplied buffer fits. `Data(id, bytes, flow_bytes, end)` carries body
octets separately from the flow-control cost, which includes padding.
`consumed(id, flow_bytes)` returns credit only for octets already received on
that stream. `ping`, `reset` and `goaway` produce the corresponding control
frames. SETTINGS window reductions can make existing stream windows negative;
new DATA waits until sufficient credit arrives. GOAWAY stops new local streams.
After sending GOAWAY, a server refuses later requests while still decoding their
header blocks to preserve compression state for earlier active streams. Its last
stream ID identifies a request whose validated headers were delivered.

`connection(net, take stream, role, header_limit)` wraps this endpoint around a
branded core Stream. It is a prior-knowledge API: plain streams use h2c; a TLS
stream must already have its protocol selected by the caller. It uses the
existing Net provider selection in RunConfig (`net.stream@1`, `net.script@2`,
or replay), with no new transport or retry policy. `start`, `write_headers`,
`write_body`, `release_body` and `reset_stream` apply the corresponding endpoint
operation and flush its output. `next_event` retains partial frame bytes,
verifies the server-side preface and flushes protocol responses before returning.
A failed write makes the connection unusable; EOF within a frame or preface is
`Truncated`. Dropping the connection closes its owned stream.

The explicit modules do not negotiate ALPN themselves. The `http` adapter maps
request/body operations onto their Session for explicit h2c; automatic HTTPS
selection still requires the TLS ALPN provider integration.

## Unicode scalar properties and simple case

The pure allocation-free `std@1/unicode` helpers use the same pinned Unicode
16.0.0 data as NFC and grapheme segmentation:

```text
fn unicode.is_whitespace(c: char) -> bool uses {}
fn unicode.is_letter(c: char) -> bool uses {}
fn unicode.is_digit(c: char) -> bool uses {}
fn unicode.simple_lower(c: char) -> char uses {}
fn unicode.simple_upper(c: char) -> char uses {}
```

Whitespace is the Unicode White_Space property, letters are general categories
L*, and digits are Nd. Unmapped scalars pass through the case functions
unchanged. Simple case
maps one scalar to one scalar, without full string casing, casefold or locale
rules. Generated immutable ranges and mapping rows come from the vendored
UnicodeData.txt and PropList.txt; `stdlib/unicode16/generate.py --check` verifies
byte-identical normalization, property and full case table regeneration.

## Bounded CSV

`std@1/csv` is an ordinary-source incremental CSV parser and writer. Its public
`Config` has `delimiter: u8`, `fields_per_record: Option[u64]` and
`max_record_bytes: u64`; `default_config()` chooses comma, no width constraint,
and 1 MiB per record. `CsvError = Syntax(u64) | UnequalFields(u64) |
RecordTooLarge | InvalidUtf8 | InvalidDelimiter`, and
`ReadError[E] = Source(E) | Csv(CsvError) | InvalidRead`.

```text
fn csv.reader(config: Config) -> Result[Reader, CsvError] uses {}
fn csv.feed(mut reader: Reader, chunk: Bytes, eof: bool) -> Result[List[List[String]], CsvError] uses {}
fn csv.parse(take text: String, config: Config) -> Result[List[List[String]], CsvError] uses {}
fn csv.read_record[S,E](mut reader: Reader, mut source: S, refill: fn(mut S, mut List[u8], u64) -> Result[u64,E]) -> Result[Option[List[String]], ReadError[E]]
fn csv.write_record(fields: List[String], delimiter: u8, mut out: List[u8]) -> Result[(), CsvError] uses {}
```

Delimiters must be nonzero ASCII other than quote/CR/LF. Fields may be quoted,
contain delimiters or newlines, and escape a quote by doubling it. A quote in an
unquoted field or data after a closing quote is syntax error. Outside quotes,
LF, CR and CRLF terminate records; quoted CRLF is preserved as data, matching
`examples/csvjson`. Empty fields and blank records (one empty field) are
preserved; empty input has no records, and a final terminator adds no extra row.
EOF finalizes an unterminated record once, rejecting an unclosed quote. Later
empty feeds are harmless; a nonempty feed after EOF returns Syntax at EOF.

The record limit counts raw bytes, including quotes and delimiters but excluding
an outside-quote record terminator. It is checked before retaining another byte;
zero allows empty records only. Width is checked per record when configured.
Syntax offsets are absolute input byte offsets (EOF when a quote is unclosed);
UnequalFields reports the record's starting byte offset. UTF-8 may cross chunks,
but every completed field must validate. Header uniqueness is application policy.
Parser errors are terminal; subsequent calls report Syntax at the current offset.

`feed` returns all complete rows from its explicit chunk, allocating in proportion
to that chunk. `parse` uses the same transitions. `read_record` parses only through
one returned row and retains bounded raw lookahead, with no completed-row queue.
Refills request at most 4096 bytes and at most the current record's remaining
limit plus one; the extra byte permits a terminator at the exact limit. Refills
append exactly their returned count and must not exceed the request; violating
that contract yields InvalidRead. Source errors propagate as Source and end the
reader. None occurs only at clean EOF and repeated reads never refill after EOF.
Callers should continue with read_record while it has raw lookahead; feeding
external chunks while lookahead remains is a Syntax/state error.

The writer validates the delimiter before mutating output (Strings are already
valid UTF-8), quotes fields only for delimiter/quote/CR/LF, doubles quotes and
emits exactly one LF. An empty field list writes a blank record. It appends to
existing output. No file, stdin or network hook is hidden in the parser; refill
carries the explicit source's authority, like bufio.

## Explicit argument flag parsing

`std@1/cli` parses a supplied argument snapshot as pure source. It defines
`Kind = Switch | Value`, `Flag { name: String, short: Option[u8], kind: Kind,
help: String }`, and `Parsed { flags: Map[String, List[String]], positional:
List[String] }`. `ParseError = UnknownFlag(u64) | MissingValue(u64) |
UnexpectedValue(u64) | InvalidSpec`; indices identify arguments, starting at zero.

```text
fn cli.parse(take args: List[String], specs: List[Flag]) -> Result[Parsed, ParseError] uses {}
fn cli.usage(program: String, specs: List[Flag]) -> String uses {}
```

Specs require nonempty long names without a leading dash, `=`, ASCII control,
DEL or space, and unique names/short options. A short option is one printable
ASCII byte other than dash or `=`. `parse` rejects invalid specs before input.
Long flags accept `--name=value` or `--name value`; shorts accept `-x`,
`-x=value` or `-x value`, without bundling. A value flag consumes its next token
even if that token starts with a dash or is `--`. Switches reject explicit values
and store an empty-string occurrence. Repeats retain occurrence order. Unknown
flags fail; a lone dash is positional. Positionals may intermix with flags;
a standalone `--` ends option parsing and is omitted from the result.

`usage` requires valid specs (otherwise SEM-3 failure), emits a Usage line and
one line per spec in declaration order, and appends LF. Applications collect
Args through arg.len/text once, parse typed values separately, and choose how
to print help or errors. There is no registration, ambient Args access or exit.

## Explicit-sink structured logging

`std@1/log` defines `Level = Debug | Info | Warn | Error`, ordered in that
severity order, `Field { name: String, value: Value }`, and
`Entry { level: Level, message: String, fields: List[Field], timestamp:
Option[time.Timestamp] }`.

```text
fn log.to_string(entry: Entry) -> String uses {}
fn log.write(sink: Stderr, minimum: Level, entry: Entry) -> Result[(), OutputError]
```

`Value` is `Null | Text(String) | Int(i64) | Uint(u64) | Float(f64) | Bool(bool) |
Duration(time.Duration) | Timestamp(time.Timestamp) | Json(json.Value) |
Failure(String) | Severity(Level)`. Applications pass structured JSON explicitly
with `Json` and error text with `Failure`; there is no reflection or callback.

`to_string` emits Go slog JSONHandler's compact JSON object plus one LF: `time`
(RFC 3339 in UTC, when supplied), `level` (DEBUG/INFO/WARN/ERROR), `msg`, then
flat field key/value attributes in list order. Duplicate keys, including built-in
names, are preserved. Only a field with an empty name and `Null()` is omitted.
`None()` omits time; Unix epoch zero remains a real timestamp. Strings use slog's
escaping (including U+2028/U+2029) without HTML escaping. Numbers and booleans
remain JSON scalars, durations are integer nanoseconds, timestamps use RFC 3339,
and failures and severities are strings. Nonfinite floats and encoding failures
become `!ERROR:` strings, leaving the rest of the record intact. Invalid JSON
numbers and nesting use Hirsel's codec error descriptions.

Timestamp is caller-supplied; no wall clock is read. Write filters below minimum
before formatting, calls err.print once with the complete record, and propagates
its error. There is no global logger, implicit sink, hidden clock or ambient
level. Ordering/atomicity across tasks is not promised; callers needing ordering
serialize through their explicit task/channel facilities. The port and vector
mapping are recorded in [the slog port notes](../../reports/log-slog-port.md).

Recorded structured observations can be exported separately with
`python3 tools/obs_export.py`; see [Separate OTLP export](determinism.md#separate-otlp-export).
It preserves explicit trace IDs and units and maps Debug/Info/Warn/Error to OTLP
5/9/13/17. `log.Entry` uses the slog JSON schema described above; stderr errors
propagate. Export is host tooling with its own endpoint and storage grants, not a
new library effect: neither `log.write` nor source replay contacts a collector.
Untimed records remain logs without a source timestamp; spans/histograms require
original timing evidence and an explicit original Unix/monotonic correlation.

## Additional testing helpers

`test.expect_ok[T,E](t: Test, take value: Result[T,E]) -> T` returns an owned
Ok payload. An unexpected Error payload is destroyed before test.fail, matching
expect_error's cleanup rule. A failing payload finalizer therefore precedes the
assertion failure. These are ordinary source operations using the existing Test
capability. The resource-valued conformance example is
`conformance/SEM/SEM-3-test-extra-run.hir`.

## SHA-256

`std@1/sha256` implements SHA-256 entirely in ordinary, allocation-free source.
Its opaque `State` contains eight chaining words, a partial 64-byte block and a
byte count. The public operations are:

```text
fn sha256.new() -> State uses {}
fn sha256.update(mut state: State, data: Bytes) -> () uses {}
fn sha256.finish(take state: State) -> Inline[u8, 32] uses {}
fn sha256.digest(data: Bytes) -> Inline[u8, 32] uses {}
fn sha256.digest_hex(data: Bytes) -> unique String uses {}
```

Update borrows bytes and accepts an empty chunk. Before changing state it checks
that total input remains at most 2^61-1 bytes, the maximum whole-byte message
whose bit length fits SHA-256's 64-bit length field; exceeding it fails SEM-1.
Finish consumes state, pads once, and emits exactly 32 bytes in big-endian word
order. Round additions wrap deliberately at 32 bits. `digest` is the one-shot
form over the same kernel. Hexadecimal rendering is a separate codec;
`digest_hex` is `hex.encode(bytes.from_inline(digest(data)))`. This is a
digest, without password-hashing or signature operations.

## SHA-1, SHA-2 family and MD5

`sha1`, `sha224`, `sha384`, `sha512`, `sha512_256` and `md5` follow the `sha256`
shape exactly, each with its own opaque `plain + inert` `State` and digest size `n`
(20, 28, 48, 64, 32 and 16 bytes):

```text
fn <hash>.new() -> State uses {}
fn <hash>.update(mut state: State, data: Bytes) -> () uses {}
fn <hash>.finish(take state: State) -> Inline[u8, n] uses {}
fn <hash>.digest(data: Bytes) -> Inline[u8, n] uses {}
fn <hash>.digest_hex(data: Bytes) -> unique String uses {}
```

All but `digest_hex` allocate nothing. SHA-1 and MD5 use SHA-256's 64-byte block,
64-bit bit-length field and 2^61-1-byte input limit; MD5 writes its length field and
output words little-endian, the others big-endian. SHA-384, SHA-512 and SHA-512/256
(FIPS 180-4) share the SHA-512 kernel with a 128-byte block and a 128-bit length
field; their byte count is a `u64`, so update fails SEM-1 once total input would
exceed 2^64-1 bytes. SHA-224 is the SHA-256 kernel and SHA-384 and SHA-512/256 the
SHA-512 kernel from their own FIPS 180-4 initial words, truncated to the leading
digest bytes; the shared kernel is one source definition, not a copy.

SHA-1 is not collision resistant (SHAttered, 2017) and MD5 is broken outright:
both exist only to read and write formats and protocols that name them (Git object
ids, legacy TLS/PGP data, `Content-MD5`, archive checksums). Neither may
authenticate data against an adversary; use SHA-256 or another SHA-2 digest.

Throughput: every hash buffers input a byte at a time into its block, then runs
one compression per full block: SHA-1 80 rounds and MD5 64 rounds over 32-bit
words, the SHA-256 kernel 64 rounds, the SHA-512 kernel 80 rounds over 64-bit
words on twice the block: 0.625 rounds per byte against SHA-256's 1.0, so on a
64-bit target SHA-512 is usually the faster kernel. Truncated variants cost exactly their
kernel. There is no bench gate.

## BLAKE2

`blake2b` is RFC 7693 BLAKE2b-512 and `blake2s` BLAKE2s-256, each in the `sha256`
shape with 64- and 32-byte digests, plus a keyed MAC form:

```text
fn blake2b.new_keyed(key: Bytes) -> State uses {}
fn blake2s.new_keyed(key: Bytes) -> State uses {}
```

A key of at most 64 (BLAKE2b) or 32 (BLAKE2s) bytes is zero-padded into the first
block, as RFC 7693 specifies; a longer key fails SEM-3 and an empty key is the
unkeyed hash. Update holds the most recent block back, because finish compresses
the last block with the final flag; the byte counter is a `u64`, so total input
is limited to 2^64-1 bytes (SEM-1 beyond). Output words are little-endian. BLAKE2
has no length-extension weakness, so its keyed form is a MAC by itself.

Throughput: one compression per block, 12 rounds of eight G mixes over 64-bit
words per 128-byte block (BLAKE2b) and 10 rounds over 32-bit words per 64-byte
block (BLAKE2s). BLAKE2b runs fewer operations per byte than SHA-512 and SHA-256
on a 64-bit target; BLAKE2s suits 32-bit ones. There is no bench gate.

## SHA-3 and SHAKE

`sha3_224`, `sha3_256`, `sha3_384` and `sha3_512` are FIPS 202 SHA3-224 to SHA3-512 in
the `sha256` shape, with 28-, 32-, 48- and 64-byte digests and their own opaque
`plain + inert` `State`. `shake128` and `shake256` are the FIPS 202 extendable-output
functions, whose output length the caller chooses:

```text
fn shake128.new() -> State uses {}
fn shake128.update(mut state: State, data: Bytes) -> () uses {}
fn shake128.finish(take state: State, length: u64) -> unique Bytes uses {}
fn shake128.digest(data: Bytes, length: u64) -> unique Bytes uses {}
```

and the same for `shake256`. Every output is a prefix of every longer output of the
same input; a length of zero is the empty `Bytes`. All six share one Keccak-f[1600]
sponge (`internal.keccak`): 24 rounds over 25 64-bit lanes, at a rate of 144, 136, 104
and 72 bytes for SHA3-224 to SHA3-512 and 168 and 136 bytes for SHAKE128 and SHAKE256.
Input is absorbed into little-endian lanes, a whole lane at a time where it is
lane-aligned, and output squeezed a byte at a time. SHA-3 pads with the domain byte
0x06 and SHAKE with 0x1f, each followed by the final 0x80 bit of pad10*1. The sponge
has no length field, so update accepts any total length. Keccak has no
length-extension weakness. The SHA-3 digests return a `digest.Hash` from `hash()`,
whose block size is the rate, so they key HMAC, HKDF and PBKDF2.

Throughput: one permutation per rate-sized block, each round unrolled over the lanes
in locals, so SHA3-512 costs nearly twice SHA3-256 per byte and SHAKE128 least; Keccak
is slower in software than SHA-256 or BLAKE2 per byte. There is no bench gate.

## Checksums

`crc32` (IEEE 802.3, reflected polynomial 0xedb88320: Ethernet, gzip, zip, PNG),
`crc32c` (Castagnoli, 0x82f63b78: iSCSI, SCTP, ext4), `adler32` (RFC 1950, zlib) and
`xxhash64` (XXH64) detect accidental corruption; none resists an adversary. Each
has an opaque `plain + inert` `State` and returns an integer:

```text
fn <checksum>.new() -> State uses {}
fn <checksum>.update(mut state: State, data: Bytes) -> () uses {}
fn <checksum>.finish(take state: State) -> u32 uses {}    // u64 for xxhash64
fn <checksum>.digest(data: Bytes) -> u32 uses {}          // u64 for xxhash64
fn <checksum>.digest_hex(data: Bytes) -> unique String uses {}
fn xxhash64.new_with_seed(seed: u64) -> State uses {}
fn crc32.update_list(mut state: State, data: List[u8], start: u64) -> () uses {}
fn adler32.update_list(mut state: State, data: List[u8], start: u64) -> () uses {}
```

`update_list` extends the CRC-32 or Adler-32 with a list builder's bytes from index
`start` on (none when `start` is at or past the end), so output built in a `List[u8]`
is checked where it is, as `update` checks `Bytes`.

`digest_hex` writes the value as fixed-width lowercase hexadecimal, most
significant digit first (8 digits, or 16 for `xxhash64`), the form `cksum`-style
tools and the xxHash canonical representation print. `new()` of `xxhash64` is seed
0. CRC-32C shares CRC-32's table-driven kernel with its own table. Adler-32 sums
reduce modulo 65521 once per 5552 bytes, the longest run that cannot overflow 32
bits. xxHash64 multiplications and additions wrap at 64 bits; its total length
also wraps, as in the reference. Update accepts any length; none of these checks
input size.

Throughput: CRC-32 and CRC-32C do one 256-entry table lookup per byte (no
slicing-by-8 or carry-less multiply); Adler-32 two additions per byte and two
remainders per 5552 bytes; xxHash64 four multiply-rotate lanes per 32 bytes. They
cost a small fraction of a cryptographic digest per byte. There is no bench gate.

## Hash dictionaries, HMAC and key derivation

`std@1/digest` names a cryptographic hash as an explicit dictionary
([GEN-2](generics.md#gen-2-dictionaries)), the role Go's `hash.Hash` constructor
plays for `crypto/hmac`:

```text
pub type digest.Hash[S: plain + inert, n: size] {
  new: fn() -> S uses {},
  update: fn(mut S, Bytes) -> () uses {},
  finish: fn(take S) -> Inline[u8, n] uses {},
  block_size: u64,
}
fn <hash>.hash() -> digest.Hash[State, n] uses {}
```

`sha1`, `sha224`, `sha256`, `sha384`, `sha512`, `sha512_256`, `sha3_224`,
`sha3_256`, `sha3_384`, `sha3_512`, `blake2b` and `blake2s` each return theirs from
`hash()`, with block sizes 64, 64, 64, 128, 128, 128, 144, 136, 104, 72, 128 and 64.
`md5` has none, so it cannot key an HMAC, and the checksums have none. A record
holding functions is not a compile-time constant, hence a function.
Every operation below is generic over `[S: plain + inert, n: size]`, takes the
dictionary first, and is ordinary allocating source:

```text
fn hmac.new(hash: digest.Hash[S, n], key: Bytes) -> hmac.State[S] uses {}
fn hmac.update(hash: digest.Hash[S, n], mut state: hmac.State[S], data: Bytes) -> () uses {}
fn hmac.finish(hash: digest.Hash[S, n], take state: hmac.State[S]) -> Inline[u8, n] uses {}
fn hmac.mac(hash: digest.Hash[S, n], key: Bytes, data: Bytes) -> Inline[u8, n] uses {}
fn hmac.verify(hash: digest.Hash[S, n], key: Bytes, data: Bytes, expected: Bytes) -> bool uses {}
fn hkdf.extract(hash: digest.Hash[S, n], salt: Bytes, secret: Bytes) -> Inline[u8, n] uses {}
fn hkdf.expand(hash: digest.Hash[S, n], prk: Bytes, info: Bytes, length: u64) -> unique Bytes uses {}
fn hkdf.derive(hash: digest.Hash[S, n], salt: Bytes, secret: Bytes, info: Bytes, length: u64) -> unique Bytes uses {}
fn pbkdf2.derive(hash: digest.Hash[S, n], password: Bytes, salt: Bytes, iterations: u64, length: u64) -> unique Bytes uses {}
fn constant_time.equal(a: Bytes, b: Bytes) -> bool uses {}
```

HMAC is RFC 2104: a key longer than the block is hashed first, then the key is
zero-padded to the block and masked with 0x36 (inner) and 0x5c (outer). The opaque
`hmac.State[S]` is `plain + inert`, so copying a keyed state reuses its two
compressed pad blocks; HKDF and PBKDF2 key once and copy per block or iteration.
`hmac.verify` compares through `constant_time.equal`; comparing tags with `==`
leaks how many leading bytes match. HKDF is RFC 5869: an empty salt is the RFC's
default of `n` zero bytes (HMAC pads both identically), and `expand` fails SEM-3
when `length` exceeds 255 * n. PBKDF2 is RFC 8018 with HMAC as the PRF: block
indices are big-endian 32-bit, `iterations` below 1 and `length` above
(2^32 - 1) * n fail SEM-3. PBKDF2 is a password-based key derivation function of
the 2000s; choose its iteration count for the deployment, as current guidance
asks for hundreds of thousands with SHA-256.

**Constant time.** `constant_time.equal` returns whether `a` and `b` have equal
length and contents. Lengths are public: unequal lengths return false at once. For
equal lengths it reads every byte pair, ORs their XOR into one accumulator, and
tests the accumulator once after the loop: there is no early exit, and the
executed operation sequence (which instructions run, in which order, how many
times) depends only on the length, never on the contents or on where they differ.
This is the property Go's `crypto/subtle.ConstantTimeCompare` states; it promises
nothing about what a native code generator or processor does below the operation
sequence. The driver test `stdlib_constant_time` checks it on the reference
interpreter: the full instruction and terminator trace and the step count of
`equal` on 32 equal bytes are identical to those when the inputs differ at the
first, middle or last byte, and its control shows an early-exit comparison changes
that trace.

Throughput: HMAC costs two extra compressions per key (the pads, done once per
`new`), plus two finishes per tag; HKDF one HMAC per output block; PBKDF2 two
compressions per iteration per output block, which is its intended cost.
`constant_time.equal` is one pass. There is no bench gate.

## Authenticated encryption

`chacha20poly1305` is RFC 8439 ChaCha20-Poly1305, `xchacha20poly1305` its
extended-nonce variant (draft-irtf-cfrg-xchacha-03) and `aes_gcm` AES (FIPS 197) in
Galois/Counter Mode (NIST SP 800-38D). Each seals a plaintext with a key, a nonce and
associated data, and opens what it sealed:

```text
pub type aead.OpenError = AuthenticationFailed()
fn <aead>.seal(key: Bytes, nonce: Bytes, plaintext: Bytes, data: Bytes) -> unique Bytes uses {}
fn <aead>.open(key: Bytes, nonce: Bytes, sealed: Bytes, data: Bytes) -> Result[unique Bytes, aead.OpenError] uses {}
```

A sealed message is the ciphertext, as long as the plaintext, followed by a 16-byte
tag over the ciphertext and `data`; `data` is authenticated but not encrypted. Keys
are 32 bytes for both ChaCha20 forms and 16, 24 or 32 for `aes_gcm`, whose key length
selects AES-128, AES-192 or AES-256. Nonces are 12 bytes, and 24 for
`xchacha20poly1305`. Any other key or nonce length fails SEM-3: these are the caller's
own values, never the peer's. `open` recomputes the tag, compares it with the received
one through `constant_time.equal`, and decrypts only when they match; otherwise, and
for a sealed message shorter than a tag, it returns `AuthenticationFailed()` and no
plaintext. A nonce must never repeat under one key: reuse reveals the XOR of two
plaintexts and, for Poly1305 and GHASH, lets the authenticator be forged. A 24-byte
XChaCha20 nonce is long enough to choose at random; a 12-byte one is better a counter.

ChaCha20-Poly1305 encrypts from block counter 1, keys Poly1305 with the first 32 bytes
of block 0, and authenticates the data and ciphertext each zero-padded to 16 bytes,
then both lengths as 64-bit little-endian words. A plaintext longer than (2^32 - 1) *
64 bytes, the 32-bit counter's reach, fails SEM-1. XChaCha20-Poly1305 is
ChaCha20-Poly1305 under the HChaCha20 subkey of the key and the nonce's first 16
bytes, with a 12-byte nonce of four zero bytes and the nonce's last 8. AES-GCM keys
GHASH with H = AES(key, 0^128), encrypts from counter block nonce || 2 and masks the
tag with the encryption of nonce || 1; a plaintext over 2^36 - 32 bytes or data over
2^61 - 1 bytes fails SEM-1 when sealing and does not open. Tags are always 16 bytes;
there are no truncated tags and no other nonce sizes.

Throughput: ChaCha20 runs 20 rounds per 64-byte block and Poly1305 one 130-bit
multiplication per 16 bytes. AES works on bit planes, four blocks at once: plane k
holds bit 7 - k of all 64 state bytes, so SubBytes is one pass of Zig's 113-gate
Boolean S-box circuit, and the counter blocks are encrypted four per pass (the first
pass also yields GHASH's key and the tag mask). GHASH multiplies with Zig's masked
integer carry-less products, three per 16 bytes, and Gueron's reduction. AES-GCM
remains the slowest of the three without hardware AES. There is no bench gate.

**Side channels.** These modules are written so their operation sequence and memory
access pattern do not depend on secrets: secret-dependent choices are mask-selects,
never branches or indices (Poly1305's final reduction, AES SubBytes as a Boolean
circuit instead of a table, GHASH's masked multiplication), and tags are compared
through `constant_time.equal`. Length checks are variable-time in public lengths only.
As for `constant_time.equal`, this is a property of the source operation sequence, and
the driver test `stdlib_constant_time` checks it on the reference interpreter:
`aes_gcm.seal` and `chacha20poly1305.seal` under two different keys and plaintexts of
one length run identical instruction traces and step counts. The reference interpreter
is not constant time: its dispatch, allocation and integer representation take time
that can depend on values, and it makes no promise about caches. Native code
generators and processors may also introduce branches or variable-time instructions.
Nothing here erases secrets from memory after use: copies of keys may remain in freed
storage. Use these modules where those limits are acceptable, and a vetted native
library through FFI where they are not.

## FNV-1a hashing

`std@1/fnv` implements 64-bit FNV-1a in ordinary, allocation-free source, with the
`new`, `update` and `finish` shape of the other std checksums. `fnv.new()` holds the
offset basis; `fnv.update(mut state, data)` folds each byte in (xor, then multiply
by the FNV prime modulo 2^64); `fnv.finish(take state)` returns the `u64` hash, and
`fnv.digest(data)` is the one-shot form. It is a fast non-cryptographic hash for
tables and fingerprints, not a cryptographic digest.

## Derived bodies

`std@1/derive` holds generators for `= comptime` function bodies
([CT-5](compile-time.md#ct-5-generation)). Each takes a record or variant type's
shape and returns the body for the header written in source:

```text
fn derive.json_value[T](s: TypeShape[T]) -> Result[Body[fn(T) -> json.Value uses {}], String] uses {}
fn derive.from_json[T](s: TypeShape[T]) -> Result[Body[fn(json.Value) -> Result[T, String] uses {}], String] uses {}
fn derive.equal[T](s: TypeShape[T]) -> Result[Body[fn(T, T) -> bool uses {}], String] uses {}
fn derive.format[T](s: TypeShape[T]) -> Result[Body[fn(T, mut List[u8]) -> () uses {}], String] uses {}
fn derive.hash[T](s: TypeShape[T]) -> Result[Body[fn(T, mut siphash.State) -> () uses {}], String] uses {}
```

```text
fn todo_value(t: Todo) -> json.Value uses {} = comptime derive.json_value(shape(Todo))
```

Field and payload types may be `bool`, `String`, the sized integers, `f32`,
`f64`, another record or variant type, and `Option`, `List`, `Set`, `Map` and
tuples of supported types; any other is the generator's `Error`, `no JSON form
for T`, `no text form for T` or `no hash form for T`, which CT-5 reports as
CT-1 at the declaration.

A field of another record or variant type `Item` (any package, generic
instances included) uses `Item`'s own deriver, named from its declared name in
snake case: `item_to_json`, `item_from_json`, `item_format` and `item_hash`
(`TodoItem` gives `todo_item_to_json`, `HTTPRequest` gives
`http_request_to_json`). Every generated nested call is qualified with the field
type's canonical package and module, so a field `model.Draft` calls
`model.draft_to_json`, `model.draft_from_json`, `model.draft_format` or
`model.draft_hash`, without forwarding functions in the enclosing record's
module. Ordinary visibility applies. The deriver may be derived or written by
hand, and a missing one is LANG-4 in the generated text. A naming convention keeps
headers exactly what their writers declare, where a dictionary argument would
add a parameter to every derived function and its callers. A type that recurses
through a `List`, `Set` or `Map` gets a deriver that calls itself. A `box T`
field has no form: a deriver takes its type by value and a box's payload is
reached only through patterns ([MEM-7](memory.md#mem-7-layout)). `equal`
compares every field with `==`, nested types included.

A generic declaration derives each instance separately (CT-5 instances), so one
declaration serves every instance of a generic type, and a nested `T` field uses
the deriver of each instance's argument:

```text
fn page_to_json[T](p: Page[T]) -> json.Value uses {} = comptime derive.json_value(shape(Page[T]))
```

A decoder declared `-> Result[unique T, String]` builds every nested value it
holds through its nested types' decoders, so they are declared `unique` too.

A record's JSON form is an object with a member per field, in declaration
order; integers are JSON numbers, and so are finite floats, written by
`to_string`; `NaN`, `inf` and `-inf` are those JSON strings. `None` is `null`,
so `Option[Option[T]]`, at any depth, has no JSON form. A list, a set and a
tuple are arrays; a `Map[String, V]` is an object and any other map an array of
`[key, value]` arrays. A variant is externally tagged, as serde writes it
(serde_derive `serialize_externally_tagged_variant`, serde_json
`serialize_*_variant`): a variant without payload is its name as a JSON string,
and any other is an object whose single member, its name, holds a single
positional payload itself, the array of two or more positional payloads, or the
object of labelled payloads:

```text
type Animal = Dog | Frog(String, List[i64]) | Cat(age: u64, name: String) | AntHive(List[String])
Dog                         "Dog"
Frog("Henry", [349, 102])   {"Frog":["Henry",[349,102]]}
Cat(age: 5, name: "Kate")   {"Cat":{"age":5,"name":"Kate"}}
AntHive(["Bob", "Stuart"])  {"AntHive":["Bob","Stuart"]}
```

`from_json` reads that form back with serde_json's rules for a decoded value
(`serde_json::from_value`). A unit variant is also read from `{"Name": null}`.
An object holding a variant has exactly one member. A positional variant's
array has exactly its payload count, and a labelled variant needs an object.
Labelled payloads and record fields decode alike: an absent member of an
`Option` field is `None`, any other absent member is an error, and unknown
members are ignored. A duplicate set element or map key, and any other mismatch,
is `Error`. Structural errors carry serde's messages without a text position,
such as `unknown variant `x`, expected one of `Dog`, `Frog`, `Cat`, `AntHive``,
`invalid type: unit variant, expected tuple variant`, `invalid type: map,
expected unit`, `invalid length 1, expected tuple variant Animal::Frog with 2
elements`, `missing field `age`` and `invalid type: string "x", expected struct
Todo`. Its body builds every value fresh, so a header may
declare `-> Result[unique T, String]` (CT-5 decides uniqueness from the header).
`json_into` writes the same text as `json_value` followed by
`json.stringify_into`, under the same nesting limit.

**Format note (2026-10-20).** Derived JSON variants used to be an object whose
member held the array of all payloads, `{"Circle":[2]}` and `{"Rect":[3,4]}`
for `Circle(u32)` and `Rect(width: u32, height: u32)`. They are now serde's
externally tagged form, `{"Circle":2}` and `{"Rect":{"width":3,"height":4}}`,
so derived JSON interoperates with Rust serde types of the same shape. Unit
variants, records and every non-variant form are unchanged. The old form is
not read. A derived `hash` used to be FNV-1a over formatted text; it now
streams into SipHash-1-3, so every hash value changed.

`format` writes `Name(field: value, ..)` for a record and `Variant(payload, ..)`
or `Variant` for a variant; strings are written unquoted, `Option` as `Some(..)`
or `None`, lists as `[a, b]`, sets as `{a, b}`, maps as `{k: v}` and tuples as
`(a, b)` or `(a,)`. Its header is that of an `@format` function.

`hash` streams a value into a `std@1/siphash.State`, as Rust's
`#[derive(Hash)]` streams into a `Hasher`. A record writes its fields in order.
A variant of a type with more than one variant writes its declaration index as
a `u64` and then its payloads. Integers and `bool` are their little-endian
bytes, a `String` its bytes and then 0xFF, a list its length and then each item,
a map its length and then each key and value in order, an `Option` its index
(`None` 0, `Some` 1) and payload, and a tuple each component. A nested type
streams through its own `_hash`. A set, whose equality ignores order, writes
its length and then the wrapping sum of each element's hash under the same
keys, so equal sets hash alike. Floats are not `key` ([GEN-1](generics.md#gen-1-bounds))
and have no hash form. `siphash.hash_one(value, item_hash, k0, k1)` returns the
hash. For a shape Rust can spell, it equals Rust's under `SipHasher13` with
those keys, and keys 0 and 0 give `DefaultHasher::new()`'s. The keys are always the
caller's, never the run's hidden placement seed ([SEM-5](semantics.md#sem-5-maps)),
so a hash is the same in every run. `json_bool`, `json_text`, `json_number`,
`json_float`, `json_float_text`, `parsed`, `json_unexpected`,
`json_invalid_type`, `unknown_variant` and `json_length` are the helpers
generated bodies call.

`std@1/siphash` is SipHash-1-3 after Rust's `core::hash::sip`, the hash behind
Rust's `DefaultHasher` and the runtime's keyed table hash.
`siphash.new(k0, k1)` starts a state. `write(mut state, data)`,
`write_str` (bytes, then 0xFF), `write_bool` and `write_u8` through
`write_i128` (little-endian bytes) feed it, and how input is split across
writes never changes the hash. `finish(state)` returns the `u64` hash without
consuming the state. `fresh(state)` starts a new state under the same keys, and
`digest(k0, k1, data)` is the one-shot form. It is a keyed hash for tables, not
a cryptographic MAC.

## ASCII radix digits

`ascii.digit_value_radix(c: u8, radix: u8) -> Option[u8] uses {}` accepts
radices 2 through 36 and decimal ASCII digits plus both ASCII letter cases.
Values outside the radix, invalid radices, and non-ASCII bytes return None.
It shares the decimal `digit_value` and ASCII case kernel; the existing decimal
helper retains its signature. `ascii.is_space` remains the sole ASCII-whitespace
predicate (space and HT/LF/VT/FF/CR); split_ascii_whitespace retains its documented
exclusion of VT. Hex/JSON/URI/network parsing and string ASCII helpers reuse these
byte kernels rather than retaining private digit/classification copies.

## Common string helpers

`string.contains(s: String, pattern: String) -> bool uses {}` delegates to
`find`; the empty pattern matches every string. `string.lines(take s: String)
-> List[String] uses {}` returns owned LF/CRLF-separated lines without their
terminators, preserves interior empty lines, and omits an extra trailing empty
line. Empty input has no lines; a bare CR remains data.
`string.replace(s: String, old: String, new: String) -> unique String uses {}` borrows
inputs and returns owned text, replacing nonoverlapping matches left to right.
An empty pattern inserts `new` at every UTF-8 scalar boundary, including both
ends (once for an empty input). All three helpers are ordinary source.

## UTF-8 construction and lossy conversion

A char's UTF-8 encoding is appended by `char.format_into` ([scalars](#scalars));
a `char` is always a scalar value, so encoding cannot fail, and `char.from_u32`
rejects surrogates and values above U+10FFFF beforehand. `utf8.decode_lossy(data: Bytes) -> unique String uses {}` borrows arbitrary bytes and
returns owned UTF-8. It replaces each invalid sequence with U+FFFD using Rust's
maximal valid-prefix/error-span policy: a valid but incomplete prefix is replaced
once; an invalid next byte is reconsidered as the start of the next sequence.
Overlong encodings, surrogate encodings and values above U+10FFFF are invalid.
It is ordinary pure source. Strict conversion remains the existing
`string.from_bytes(take bytes) -> Result[String, Bytes]` operation.

## Base64 codec

`std@1/base64` provides `Alphabet = Standard | UrlSafe`, `Padding = Required |
Omitted`, and `DecodeError = InvalidByte(u64) | InvalidLength |
InvalidPadding(u64) | NonCanonical(u64)`. Its pure source functions borrow input
and return owned output:

```text
fn base64.encode(data: Bytes, alphabet: Alphabet, padding: Padding) -> unique String uses {}
fn base64.decode(text: String, alphabet: Alphabet, padding: Padding) -> Result[unique Bytes, DecodeError] uses {}
```

Encoding checks output-size arithmetic before building. Required padding emits
and requires `=` for incomplete quanta; complete quanta need no padding. Omitted
padding rejects every `=`. Decoding rejects whitespace, the other alphabet's
symbols, padding before the final quantum, and nonzero unused low bits. Errors
use input byte offsets: the first invalid byte or invalid padding placement is
reported during scanning, followed by length/padding shape validation, then the
last data byte of a noncanonical final quantum. No partial output is returned.

## Hexadecimal codec

`hex.encode(data: Bytes) -> unique String` emits lowercase hexadecimal.
`hex.decode(text: String) -> Result[unique Bytes, hex.DecodeError]` accepts both ASCII
letter cases, with `DecodeError = InvalidDigit(u64) | OddLength`. Offsets are
input byte offsets. Decoding checks digits in order; an invalid final byte
precedes an odd-length error. No partial output is returned. Both functions
are pure ordinary source and borrow their input; outputs are owned. Encoding
checks doubled size for overflow before building.

## Ordered keys

`std@1/keys` is the order-preserving key encoding `hirsel.key.v1`, after the
FoundationDB tuple layer: for keys `a` and `b` of one type, `encode(a) <
encode(b)` bytewise exactly when `a` precedes `b` in key order.

```text
@preserves_unique(out) fn keys.encode_into[K: serializable](key: K, mut out: List[u8]) -> () uses {}
fn keys.encode[K: serializable](key: K) -> unique Bytes uses {}
fn keys.decode[K: serializable](data: Bytes) -> Result[unique K, DecodeError] uses {}
fn keys.prefix_end(prefix: Bytes) -> Option[unique Bytes] uses {}
```

A key is any `serializable` value ([SER-1](serialization.md#ser-1-serializable-schemas))
whose schema holds no `Map`, `Set` or `Simd`; with one anywhere in `K`'s schema, every
`encode` and `decode` at `K` fails SEM-3, whatever the value. The encoding is directed by
the schema, so no element carries a type code:

| Schema constructor | Key bytes |
|---|---|
| `bool` | `00` for false, `01` for true |
| `u8` to `u128` | big-endian, exactly the type's width |
| `i8` to `i128` | big-endian two's complement with the sign bit inverted, exactly the type's width |
| `f32`, `f64` | the big-endian IEEE bits with every bit inverted when the sign bit is set, else the sign bit alone |
| `char` | the scalar value as a big-endian `u32` |
| `String`, `Bytes` | each byte in order, with `00` written as `00 FF`; then `00 01` |
| unit | nothing |
| tuple, record | the fields in declaration order |
| variant, including core `Option` and `Result` and `Packed[T]` | the arm's declaration ordinal as a big-endian `u32`, then its fields in order |
| `box T`; an opted-in transparent nominal or distinct type | the payload's bytes |
| `Array[T]`, `List[T]` | for each element, `01` then the element; then `00` |
| `Inline[T, n]` | the `n` elements in order |

Every encoding is self-delimiting given the schema, so a tuple's encoding is the
concatenation of its fields' and `encode_into` appends one key after another. The string
terminator `00 01` sorts below the escape `00 FF` and above the end of a shorter string,
so a proper prefix precedes its extensions whatever field follows. **Key order** follows
the same structure: integers and `char` numerically, `false` before `true`, floats by IEEE
total order (`-0.0` before `0.0`, the canonical NaN after `inf`), `String` and `Bytes`
byte-lexicographically with a proper prefix first, tuples and records lexicographically
by field, variants by arm ordinal and then payload (so `None` precedes every `Some`), and
sequences lexicographically with a proper prefix first.

`decode` accepts exactly the encodings `encode` produces, validating escapes,
terminators, sequence markers, arm ordinals and trailing bytes as well as every scalar
the canonical codec validates. Input that ends early is `Truncated`; a `00` in a string
followed by anything but `FF` or `01` is `Scalar`; a sequence marker other than `00` or
`01`, or an arm ordinal past the last arm, is `Tag`; bytes after the key are `Trailing`;
an invalid `bool`, `char` or NaN is `Scalar` and invalid UTF-8 `Utf8`, as in
[SER-2](serialization.md#ser-2-canonical-codec). Both functions go through the canonical
codec: `encode` rewrites `codec.encode(key)`'s payload under its own descriptor, and
`decode` rewrites a key into a canonical `Option[K]` payload, under the descriptor of
`codec.encode` of `None`, for `codec.decode`.

For the encoding `e` of a key's leading fields (their `encode_into` calls in order, or
`encode` of a tuple of them), the keys whose leading fields have those values are exactly
those in the half-open range `[e, prefix_end(e))`. `prefix_end(prefix)` is the least byte
string above every extension of `prefix`: `prefix` with its trailing `FF` bytes removed
and its last remaining byte incremented, or `None` when no byte remains, where the range
has no upper bound (FoundationDB's `strinc`).

## Lazy iteration

```text
@captures(it, predicate)
fn iter.filter[T](take it: Iter[T], take predicate: fn(T) -> bool) -> Iter[T] uses {it, predicate}
@captures(it)
fn iter.take_n[T](take it: Iter[T], count: u64) -> Iter[T] uses {it}
@captures(it)
fn iter.enumerate[T](take it: Iter[T]) -> Iter[(u64, T)] uses {it}
@captures(a, b)
fn iter.zip[T, U](take a: Iter[T], take b: Iter[U]) -> Iter[(T, U)] uses {a, b}
fn iter.collect[T](take it: Iter[T]) -> List[T] uses {it}
fn iter.fold[T, A](take it: Iter[T], take initial: A, take f: fn(take A, take T) -> A) -> A uses {it, initial, f}
```

These source helpers own their inputs and callbacks through core `iter.from`.
Adapters are lazy and fused: after their first `None`, later polls return `None`
without advancing either source. `take_n` (the word `take` is reserved) advances
at most `count` times, and zero never polls its input. Filter drops each rejected
item. Zip polls left before right; if right ends, the unmatched left item is
dropped. Enumerate starts at zero and checks its increment with SEM-1 arithmetic
before publishing each pair; exhaustion of the u64 index fails rather than wraps.
Collect and fold consume items, preserving move-only ownership. Early destruction,
callback failure and normal exhaustion release captured sources and callbacks
through their ordinary cleanup. Authority and capture bounds propagate from the
selected inputs; a file-walk pipeline retains its Dir authority.

## Terminal streams

`std@1/stream` provides an opaque, noncopyable `stream.Flow[T, E]` owning an
`Iter[Result[T, E]]`. Its private iterator cannot be accessed by callers. Use
namespace functions, as for other std containers:

```text
@captures(state, step)
fn stream.from[S, T, E](take state: S,
  take step: @preserves_captures(cursor) fn(mut cursor: S) -> Result[Option[T], E])
  -> stream.Flow[T, E] uses {state, step}
@captures(input)
fn stream.from_results[T, E](take input: Iter[Result[T, E]]) -> stream.Flow[T, E] uses {input}
@advance_only(flow)
fn stream.next[T, E](mut flow: stream.Flow[T, E]) -> Option[Result[T, E]] uses {flow}
@captures(input, f)
fn stream.try_map[T, U, E](take input: stream.Flow[T, E],
  take f: fn(take T) -> Result[U, E]) -> stream.Flow[U, E] uses {input, f}
@captures(input, predicate)
fn stream.try_filter[T, E](take input: stream.Flow[T, E],
  take predicate: fn(T) -> Result[bool, E]) -> stream.Flow[T, E] uses {input, predicate}
@captures(input)
fn stream.take_n[T, E](take input: stream.Flow[T, E], count: u64) -> stream.Flow[T, E] uses {input}
fn stream.try_fold[T, A, E](take input: stream.Flow[T, E], take initial: A,
  take step: fn(take A, take T) -> Result[A, E]) -> Result[A, E] uses {input, initial, step}
fn stream.try_for_each[T, E](take input: stream.Flow[T, E],
  take visit: fn(take T) -> Result[(), E]) -> Result[(), E] uses {input, visit}
@captures(input)
fn stream.to_iter[T, E](take input: stream.Flow[T, E]) -> Iter[Result[T, E]] uses {input}
```

Construction does no pull, source effect or task spawn. A pull executes in the
consumer's task and can suspend there. `Some(Ok(item))` transfers one item;
`Some(Error(error))` reports one terminal error. An error or `None` ends the flow
before that outcome is returned. Every later pull returns `None` without calling
the source or a callback, even if an input raw iterator would resume after its
error or end. A mutable loan serializes pulls; reuse requires an explicit factory
creating a fresh flow.

Map and filter invoke callbacks in source order, only on successful items. Filter
may consume several items per output and drops rejected items. Source, mapper,
predicate and sink errors use the same `E`; convert them explicitly to a
program-specific sum before composing. To keep per-record errors recoverable,
put a nested `Result` in the item `T`. No error is converted to clean EOF and
there is no automatic retry after partial effects.

`take_n` uses `iter.take_n` internally: zero does not pull, and stopping at the
prefix limit does not look ahead. Consuming fold and for-each stop at the first
source or callback error and destroy the owned pipeline before returning. On
success they consume through clean EOF (or the prefix limit). An ended flow
retained by its caller still owns its source and callbacks until destruction;
EOF alone is not prompt release. Ordinary finalizers determine cleanup order
and discard close errors under DROP-3/4.

Capture and advance-only contracts retain source/callback authority and locality
through the opaque wrapper; a flow over a file or socket has the same held authority as its inner iterator.
It grants no capability and cannot retain `Scope` or borrowed resources in
escaping state. A flow may prove sendable per value; a receiver-backed flow does
not. The wrapper exports no `noalloc_drop` bound, so destruction allocation is
conservative even when the inner iterator has a stronger receipt. An explicit
`to_iter` conversion currently also loses positive crossing evidence: a pure
flow may prove sendable while its converted iterator does not. This is a
conservative checking limitation, with no additional runtime work. The mutable
source callback must preserve its incoming capture contract. Concrete resource callbacks that need branded mutable reads can use
`from_results(iter.from(take state, next))`, with an ordinary concrete
`next(mut State) -> Option[Result[T, E]]`, rather than claim a preservation
promise their operations do not provide. Generic callback calls make no
universal unique-output or zero-copy promise. No implicit tasks, channels, capacity, cancellation checkpoints or new
tape events are introduced. An infinitely rejecting pure filter may not yield;
cancellation-aware bounded parsing belongs in a scoped consuming driver.

`to_iter` explicitly leaves the error-aware namespace. It retains the flow's
terminal pull behavior and source/callback authority, but raw iterator operators treat
`Result` as ordinary item data: a caller can then filter out an error. Raw
`Iter[Result[T, E]]` alone makes no terminal-error guarantee; `from_results`
normalizes it before exposing a `Flow`. Pure iteration remains `Iter[T]`.

## Bounded byte streams

The `stream` namespace also provides owned byte adapters over `Flow`:

```text
fn stream.chunk_limit(bytes: u64) -> Result[stream.ChunkLimit, stream.StreamConfigError] uses {}
@captures(dir, file)
fn stream.file_chunks(dir: Dir, take file: File of dir, limit: stream.ChunkLimit)
  -> stream.Flow[Bytes, FileError] uses {dir, file}
@captures(net, socket)
fn stream.net_chunks(net: Net, take socket: Stream of net, limit: stream.ChunkLimit)
  -> stream.Flow[Bytes, NetError] uses {net, socket}
@captures(request)
fn stream.http_chunks(take request: http.Request, limit: stream.ChunkLimit)
  -> stream.Flow[Bytes, http.HttpError] uses {request}
```

`ChunkLimit` is opaque, copyable, inert checked data. `chunk_limit` rejects zero
and values above 65536 with `InvalidChunkLimit()`, before any I/O. The ceiling is
explicit library policy, independent of providers and engines. Construction of
an adapter neither reads nor spawns. Each pull makes one bounded read against the
same owned handle; it never reopens a path. It yields `1..limit` bytes, including
positive short reads, or ends at a zero read. Errors, including cancellation,
remain terminal errors rather than EOF. UTF-8 code points and application records
can cross chunks; these adapters do no decoding, framing or rechunking. A UDP
zero-length datagram requires a separate message adapter.

`http_chunks` takes a Request whose response head has already been received. It
calls `http.read_body`, preserves framing and truncation errors, and yields raw
body bytes without decompression. It owns the Request and discards trailers.
Callers needing trailers or connection recovery keep the Request and use the
manual read/trailers protocol. Ended retained flows still own their handles;
consuming a prefix through a sink destroys the complete pipeline at return.

The byte sinks are consuming terminal operations:

```text
type stream.WriteError[E, W] = Source(E) | Write(W) | InvalidWrite
fn stream.write_to[S, E, W](take input: stream.Flow[Bytes, E], take destination: S,
  write: fn(mut S, Bytes) -> Result[u64, W]) -> Result[S, stream.WriteError[E, W]]
  uses {input, destination, write}
fn stream.file_sink[E](dir: Dir, take destination: AtomicFile of dir,
  take input: stream.Flow[Bytes, E]) -> Result[(), stream.WriteError[E, FileError]]
  uses {dir, destination, input}
fn stream.net_sink[E](net: Net, take socket: Stream of net,
  take input: stream.Flow[Bytes, E]) -> Result[(), stream.WriteError[E, NetError]]
  uses {net, socket, input}
fn stream.http_upload[E](take request: http.Request, take input: stream.Flow[Bytes, E],
  trailers: List[http.Header]) -> Result[http.Request, stream.WriteError[E, http.HttpError]]
  uses {request, input}
```

`write_to` borrows each remaining suffix to its writer and repeats partial writes
until the chunk is finished, before the next source pull. Empty input chunks are
skipped. A callback returning zero for nonempty input, or more than offered,
returns `InvalidWrite()`; an error may follow an accepted prefix. Source and
writer errors are separate typed outcomes. On clean EOF the destination is
returned for explicit finish/commit; on error both destination and source are
destroyed. Callbacks are scoped to the call and can suspend; an arbitrary callback
can allocate or retain data outside the sink's own accounting.

`file_sink` uses atomic-file `write_all` and commits only after clean upstream
EOF. Source/write errors abort through drop, preserving the old destination.
`net_sink` uses the existing partial-write loop in `net.write_all` and closes its
owned socket on every exit. `http_upload` writes each chunk completely through
`http.write_request_body`, calls `end_request_body` with the explicit trailers
only after clean upstream EOF, and returns the Request for receiving its response.
Source/write errors drop the Request without a successful terminator. Finish and
commit errors are `Write`; drop discards close errors under DROP-3/4. A prefix
flow's clean end deliberately finishes its prefix upload or file output.

These operations introduce no queue, parser carry, implicit task or tape event.
A source uses a fresh byte-list builder for each read and transfers the converted
Bytes to its caller. It retains no previous chunk. A sequential sink retains at
most one complete input chunk, including its full backing allocation while a
suffix is being written; a small slice does not reduce that charge. Conversion
can temporarily hold both the initialized builder bytes and the output bytes;
builder spare capacity, metadata, provider scratch and user-held aliases are
additional storage. HTTP additionally retains its Request's existing bounded
head/framing/transport buffers (and HTTP/2 frame state), independent of chunk size.
Thus a chunk ceiling alone is not a total heap/RSS ceiling or a bound on arbitrary
source/writer callbacks. Framing/CSV/JSON carry and output-record limits belong to
explicit bounded parsing operations. None of these APIs collects an unbounded
stream. Requests, accepted prefixes, chunk boundaries, errors and destruction
follow the existing core contracts and replay unchanged.

## Bounded regular expressions

`regex` compiles immutable source patterns and searches strings without authority:

```text
pub type regex.CompileError = Syntax(u64) | ProgramTooLarge
pub type regex.MatchError = WorkLimit()
pub type regex.Span { start: u64, end: u64 }
fn regex.compile(pattern: String, max_instructions: u64) -> Result[regex.Regex, regex.CompileError] uses {}
fn regex.find(re: regex.Regex, text: String, work_limit: u64) -> Result[Option[regex.Span], regex.MatchError] uses {}
fn regex.is_match(re: regex.Regex, text: String, work_limit: u64) -> Result[bool, regex.MatchError] uses {}
```

The fixed dialect operates on Unicode scalars. It supports literal scalars, `.`,
`^`, `$`, `\A`, `\z`, bracket ranges and negation (`[a-z]`, `[^a-z]`),
structural groups `(...)` and `(?:...)`, alternation `|`, and repetitions `*`,
`+`, `?`, `{n}`, `{n,}`, `{n,m}`. Counts are unsigned decimal; a maximum must
be at least its minimum. Repetitions are greedy unless followed by `?`.
Empty groups and alternatives are valid. A bracket's initial `]` is literal;
`-` is literal when it does not introduce a range. Reversed ranges are errors; range
endpoints must be single scalars. Backslash escapes punctuation or a literal
non-ASCII scalar; `\a`, `\f`, `\n`, `\r`, `\t`, `\v` denote ASCII controls.

`\d` is ASCII 0–9; `\w` is ASCII letters, digits and underscore; `\s` is
HT, LF, FF, CR and space (excluding VT, following Go regexp). Uppercase
`\D`, `\W`, `\S` complement these classes, including inside brackets.
`\b` and `\B` assert an ASCII word boundary or its absence outside brackets.
Dot excludes LF. Anchors are absolute text boundaries, so `$` does not match
before a trailing LF. There are no flags, captures, backreferences,
lookaround, property or POSIX named classes, octal/hex escapes, replacement
language or native regex hook. An unescaped `{` begins a repetition only when
the complete `{n}`, `{n,}` or `{n,m}` form follows, with no leading zeroes
except the count `0` itself (Go's rule). Otherwise it is literal: `a{2`,
`a{,2}` and `a{01}` match their own spelling. Complete repeats with reversed
bounds remain errors. Literal braces can also be escaped.

`Syntax` reports the unexpected scalar's UTF-8 byte offset, or pattern byte
length for unexpected EOF. A repeated quantifier reports the second operator;
a bad range reports its upper endpoint; reversed repetition bounds report
the byte after the closing brace. Counts exceeding u64 yield ProgramTooLarge.
The size limit includes the final match, jumps and branch instructions.
A consuming class uses one slot for its first range and one for each further
range, bounding range payloads as well as instructions. Every intermediate
fragment must fit; sizes use checked arithmetic before repetitions expand.
Zero limits reject every pattern. Compilation uses O(pattern scalars + size
limit) memory and explicit parser/compiler stacks.

`find` returns the leftmost-first whole match, preferring earlier alternatives
and each repetition's greediness. `Span.start` is inclusive, `end` exclusive;
both are byte offsets relative to the supplied String, on scalar boundaries.
Empty matches, including EOF, return equal offsets. Groups affect structure
and priority but expose no captures. `is_match` applies the same search and
work budget, returning whether a span exists. Searches never change Regex and
own their queues; a WorkLimit error leaves it reusable.

The ordered Pike VM has no recursive backtracker. For each scalar boundary,
each instruction enters its epsilon closure at most once; first arrival wins.
Once a match is queued, lower-priority epsilon branches, threads and new
search starts are discarded; earlier consuming threads may still extend it.
The work counter starts at work_limit and charges one unit for each queue
slot initialization, each scalar boundary processed (including EOF), each
instruction popped from the explicit epsilon stack (including duplicates),
each ready instruction examined, and each character-class range tested.
Literal matching tests its one range. Input uses the existing lazy `string.scalars` view. Queue and
range iteration cannot continue after the next charged step would exceed the
budget: the call returns WorkLimit without a partial span. This accounting is
deterministic and independent of wall time or host regex behavior. Queue
memory is O(compiled size); work is bounded by the caller even on adversarial
alternation, nested repetition or nullable cycles.

For extraction, turn the returned byte offsets into `string.position` values
and slice the same text; the scalar-boundary span guarantees both positions
exist. Advancing a repeated search after an empty match is the caller's job:
advance by one scalar, and stop after an empty EOF match.

## Slash glob patterns

`glob` compiles immutable patterns and matches pure strings without authority:

```text
pub type glob.PatternError = Syntax(u64)
pub type glob.ExpandError = InvalidPattern(glob.PatternError) | Files(FileError)
fn glob.compile(pattern: String) -> Result[glob.Pattern, glob.PatternError] uses {}
fn glob.is_match(pattern: glob.Pattern, path: String) -> bool uses {}
fn glob.files(d: Dir, pattern: String) -> Result[unique List[Path], glob.ExpandError]
```

Matching covers the whole path, over Unicode scalars. `*` matches zero or more
non-slash scalars; `?` matches one non-slash scalar. `[abc]`, `[a-z]` and
`[^a-z]` match scalar classes; as in Go's `path.Match`, classes may match `/`.
A backslash escapes the following scalar, including inside classes. Classes
must be nonempty; unescaped `-` and `]` cannot be class range endpoints.
Reversed ranges are empty. Syntax offsets are UTF-8 byte offsets of the
unexpected scalar, or pattern byte length for an unexpected end.
Repeated stars have ordinary star meaning. There is no recursive `**`, brace,
tilde, environment or shell expansion, host separator conversion or path
normalization. Empty patterns match only empty paths. Matching uses two DP
rows, O(pattern scalars * path scalars) work and O(pattern scalars) space.

`files` validates and compiles once before calling `file.walk(d)`, filters its
file paths relative to the grant with the same matcher, and preserves the
walk's deterministic order. It returns files, including no inferred directory
entries. Both walk creation and per-entry errors propagate as `Files`.
`InvalidPattern` distinguishes the expansion error constructor from the opaque
`Pattern` type's constructor. Compilation and matching acquire no capability;
only `files` uses the explicit Dir.

## Directory creation

```text
fn file.mkdir_all(d: Dir, p: Path) -> Result[(), FileError]
```

Creates missing parents using confined `file.mkdir`; existing real directories
succeed, including directory symlinks resolved beneath the grant by `dir.narrow`.
An existing file returns `NotDirectory`; dangling or escaping links return their
ordinary confined lookup error. Normalization
is lexical: absolute, invalid and escaping paths return `FileFault` before
creating anything; an empty or dot path checks the granted directory and succeeds.
A later failure can leave earlier parents created. Read-only grants deny creation
through the same core operation. Listing, metadata, same-grant rename and empty
directory removal are [core operations](core-library.md#filesystem-lifecycle).


## File copying and recursive listing

```text
fn file.copy_file(d: Dir, from: Path, to: Path) -> Result[(), FileError]
fn file.list_recursive(d: Dir, p: Path) -> Result[unique List[DirEntry], FileError]
```

`copy_file` streams bytes in chunks of at most 8192 bytes through the existing file
operations, with bounded working memory independent of file size. It acquires an
atomic destination before opening the source, so a read-only Dir returns
`NotGranted` before reading any source input. Success atomically replaces the
destination; copying onto the source itself preserves its bytes. Errors abort the
temporary and leave the old destination intact. Source permissions and timestamps
are not copied. Source links use the existing confined `file.open` policy;
destination links are replaced as entries by atomic commit. No durability is promised.

`list_recursive` is an ordinary std walker over `file.read_dir`. It returns every
descendant entry, including empty directories, symlinks and other entry kinds,
in depth-first preorder with each directory's children sorted by UTF-8 bytes.
Paths are relative to `d`, including the requested prefix; the starting directory
itself is omitted. It descends only entries classified as `Directory`, never
symlinks, and rejects a starting symlink or non-directory with `NotDirectory`.
The first listing error is returned without a partial result. Listings are not a
filesystem snapshot: concurrent changes can cause lookup errors or different
entries, and all subsequent lookups retain the existing Dir confinement.

## Stores

```text
StoreConfig { durability: Durability, group_limit: u64, faults: StoreFaults, limits: StoreLimits }
StoreLimits { key_bytes: u64, value_bytes: u64, transaction_bytes: u64, range_rows: u64 }
StoreFaults = NoFaults | StopAfterWrites(u64)
StoreError = Storage(FileError) | Corrupt(String) | Fenced | Conflict | Injected | Closed
TxFailure[E] = Aborted(E) | Canceled(CancelCause) | Storage(StoreError) | Limit(TxLimit) | Unknown(StoreError)
TxLimit = KeyBytes | ValueBytes | TransactionBytes | RangeRows
CallRequest { stream: Bytes, epoch: u64, position: u64, digest: Bytes, acknowledged: u64 }
CallFailure[E] = Tx(TxFailure[E]) | Denied | Rejected
fn store.default_config() -> store.StoreConfig uses {}
fn store.transact[S, R: inert, E: inert](mut s: store.Store[S], body: fn(mut tx: store.Tx) -> Result[R, E] uses {}) -> Result[R, store.TxFailure[E]] uses {s}
fn store.call[S, R: inert + serializable, E: inert + serializable](mut s: store.Store[S], req: store.CallRequest, body: fn(mut tx: store.Tx) -> Result[R, E] uses {}) -> Result[R, store.CallFailure[E]] uses {s}
fn store.tx_get(tx: store.Tx, key: Bytes) -> Option[Bytes] in tx uses {}
fn store.tx_put(mut tx: store.Tx, take key: Bytes, take value: Bytes) -> () uses {} requires !store.reserved(key)
fn store.tx_delete(mut tx: store.Tx, take key: Bytes) -> () uses {} requires !store.reserved(key)
fn store.tx_range(tx: store.Tx, from: ordered_map.Bound[Bytes], to: ordered_map.Bound[Bytes], direction: ordered_map.Direction, limit: u64) -> List[(Bytes, Bytes)] uses {}
fn store.reserved(key: Bytes) -> bool uses {}
fn store.open(d: Dir, config: store.StoreConfig) -> Result[store.Store[Dir], store.StoreError] uses {d}
fn store.snapshot[S](mut s: store.Store[S]) -> store.Snapshot uses {}
fn store.snapshot_sequence(snap: store.Snapshot) -> u64 uses {}
fn store.get(snap: store.Snapshot, key: Bytes) -> Option[Bytes] in snap uses {}
fn store.range(snap: store.Snapshot, from: ordered_map.Bound[Bytes], to: ordered_map.Bound[Bytes], direction: ordered_map.Direction, limit: u64) -> List[(Bytes, Bytes)] uses {}
fn store.prefix_bounds(prefix: Bytes) -> (ordered_map.Bound[Bytes], ordered_map.Bound[Bytes]) uses {}
fn store.compare_keys(a: Bytes, b: Bytes) -> i32 uses {}
type store.Isolation = SnapshotIsolation | SerializableSnapshot
fn store.begin[S](mut s: store.Store[S]) -> store.Txn uses {}
fn store.begin_with[S](mut s: store.Store[S], isolation: store.Isolation) -> store.Txn uses {}
fn store.rollback[S](mut s: store.Store[S], take t: store.Txn) -> () uses {}
fn txn.put(mut t: store.Txn, take key: Bytes, take value: Bytes) -> () uses {}
fn txn.delete(mut t: store.Txn, take key: Bytes) -> () uses {}
fn txn.get(mut t: store.Txn, key: Bytes) -> Option[Bytes] in t uses {}
fn txn.range(mut t: store.Txn, from: ordered_map.Bound[Bytes], to: ordered_map.Bound[Bytes], direction: ordered_map.Direction, limit: u64) -> List[(Bytes, Bytes)] uses {}
fn txn.mark_read(mut t: store.Txn, take key: Bytes) -> () uses {}
fn store.commit[S](mut s: store.Store[S], take t: store.Txn) -> Result[store.Ticket, store.StoreError] uses {s}
fn store.sequence(t: store.Ticket) -> u64 uses {}
fn store.durable[S](mut s: store.Store[S], t: store.Ticket) -> Result[(), store.StoreError] uses {s}
fn store.compact[S](mut s: store.Store[S]) -> Result[(), store.StoreError] uses {s}
fn store.close[S](take store: store.Store[S]) -> Result[(), store.StoreError] uses {store}
```

`std@1/store` is an embedded, transactional, ordered key-value store written in Hirsel
over an explicit `Backend[S]` function dictionary and owned backend state `S`.
`open(d, config)` selects the local Dir backend; real files, Test dirs and recordings
work unchanged. Keys and values are `Bytes`, ordered
byte-lexicographically (`compare_keys`). An affine `Store[S]` is the single writer of its backend;
`Snapshot` and `Txn` are copyable values that hold no authority.

**Transactions.** `begin` starts a serializable `Txn` at the latest commit, and
`begin_with` one at either `Isolation`. `txn.get` and `txn.range` see that commit's state
with the transaction's own earlier `put`s and `delete`s applied; nothing is visible
elsewhere until `commit`. Dropping a `Txn` aborts it; `rollback` aborts it and frees its
conflict tracking at once. `commit` makes the writes the next commit, numbered by
consecutive sequences from 1, visible at once to later `snapshot`s and `begin`s, and
returns its `Ticket`. Conflicts are per key, after SlateDB's transaction manager
(`std@1/txn_manager`): `commit` returns `Conflict()` with no effect when a commit made
after the transaction began wrote a key it writes or marked with `txn.mark_read`
(`SnapshotIsolation`), or also a key it read with `txn.get` or one inside a range it
asked `txn.range` for (`SerializableSnapshot`, the default, so write skew conflicts too).
The store retains the write sets of at most 4096 recent commits or 1 MiB of their keys
while a transaction is open; a transaction older than what it retains conflicts whenever
it writes or read anything. `range` and `txn.range`
return at most `limit` entries between the bounds in `direction` order as fresh copies;
`prefix_bounds(p)` gives the bounds of exactly the keys starting with `p`. A `Snapshot`
reflects every commit up to `snapshot_sequence`, durable or not, and later commits never
change it: holding one never holds up the writer.

**Transactions with a body.** `transact` runs `body` once with a fresh `Tx`, the scoped
capability ([MODE-6](ownership.md#mode-6-scoped-capabilities)) of a transaction begun at
the latest commit; `tx_get`, `tx_put`, `tx_delete` and `tx_range` read and write through it
as the `txn` operations do. `Ok(v)` commits the writes as one commit (none when the body
wrote nothing) and returns `v`; `Error(e)` discards them and returns `Aborted(e)`; a
failure in the body discards them and propagates. Every outcome is released only once
every commit the body could see, and its own, is durable: `transact` calls `durable`
itself. A commit with a key over `limits.key_bytes`, a value over `limits.value_bytes`, or
keys and values over `limits.transaction_bytes` in total returns `Limit(..)` with no
effect. A failed storage write before the commit is `Storage(e)`; a create that failed
with a file error leaves the commit's durability to the next `open` and is `Unknown(e)`.
`transact` checks no cancellation; `Canceled` is the runner form's. Keys beginning with
`hirsel.` (`reserved`) are the store's own: `tx_put` and `tx_delete` require a key outside
them.

**Call streams.** `call` is `transact` as one exactly-once request on an ordered call
stream ([STORE-12](storage.md#store-12-transactions-at-a-runner)). The stream's row, under
`hirsel.streams/` and the stream bytes, holds its executor epoch, the last applied and
acknowledged positions, and the digest and encoded outcome of each position between them.
An epoch below the row's is `Denied()`. A position at or below the acknowledged one, past
the next one, retained under another digest, or with `acknowledged` not below it is
`Rejected()`. A retained position with an equal digest returns its recorded outcome
without running `body`. The next position runs `body`; the same commit stores the outcome,
sets `applied`, raises `acknowledged` and drops the outcomes at or below it, and an
outcome that writes nothing (`Aborted`, `Limit`) is recorded by a commit of the row alone.
A request with a higher epoch raises the row's, a duplicate's included. A storage failure
records nothing, so the caller resends the same position.

**Output gate and group commit.** Commits queue in memory until `durable` is awaited or
`group_limit` of them are pending; the group is then written as one log object, the
commit point of every commit in it. `durable(t)` returns once `t`'s commit and every
earlier one are in a written log object, writing the pending group when `t` is in it;
for a ticket already written it writes nothing. With `durability` `Durable()` (the
default) a log object is created with `file.put_new(.., Durable())` and so is synced
before `durable` returns; with `Volatile()` it is created unsynced, and a crash may lose
any suffix of the commits, never part of one. Groups are deterministic: no timers.

**Layout.** Under the `Dir`, every object is immutable and created only by `put_new`,
named by a kind directory and a 20-digit decimal id: `manifest/N` names the writer epoch,
the current snapshot object and the first log id to replay; `log/N` holds one commit
group, the first commit's sequence and each commit's ordered write set; `snap/N` holds
every entry as of one commit. Every object is a header (magic `HSTR`, format version 1,
kind, its own id, the writer epoch), a body and a CRC-32C of everything before it,
integers little-endian, so a misdirected or torn object fails its check.

**Open, recovery and fencing.** `open` creates the three directories when absent, reads
the newest manifest, loads its snapshot and replays the log objects from its first log id
in id order, applying each commit after the snapshot's. A bad last log object is a torn
tail, never acknowledged: it is removed. A bad or missing log object before the last, a
bad manifest or snapshot, or a commit sequence or writer epoch out of order refuses the
open with `Corrupt(name)`; in a `Volatile()` store the first bad or missing log object
ends the log instead, and the objects after it are removed. The opener then claims the
store: it creates an empty fence log at the next log id and then the next manifest with
the next epoch. A writer that finds the id of its next object taken has been fenced by
another writer and returns `Fenced()`. `Storage`, `Corrupt`, `Fenced` and `Injected`
close the store; every later call returns `Closed()`.

**Compaction.** `compact` writes any pending group, writes every entry into the next
snapshot object, publishes it with the next manifest (whose first log id is the next log
id), then removes older manifests and snapshots and the logs before it. Until the
manifest exists the old state is current; after it, the superseded objects are garbage
that the next `open` removes, so a crash at any point recovers.

**Storage contract.** The engine needs exactly these operations of its storage, which an
object-storage provider must offer: create an object whole if its name is absent
(`put_new`, reporting `AlreadyExists` otherwise; `Durable()` makes it survive a crash),
read a whole object, list a kind directory's names in byte order, and remove an object.
It never appends, renames, overwrites or reads at an offset. `StopAfterWrites(n)` is the
fault seam for crash tests: the store's storage mutation after its first `n` (creates,
removals and directory creations, from `open` on) returns `Injected()` with no effect.

**Backend interface.** `store.Backend[S]` contains `read(mut state: S, name: String)`
returning `Result[Bytes, FileError]`, `put_new(mut state: S, name: String, data: Bytes,
mode: Durability)` returning `Result[(), FileError]`, `list(mut state: S, prefix: String)`
returning `Result[List[String], FileError]`, and `remove` and `mkdir` with the same
state/name inputs and unit result. Every callback declares `uses {state}`. `list` returns
full relative names in byte order. The shipped dictionaries use top-level functions and put provider authority in
the state; any captured callback authority remains explicit erased evidence. `Store[S]` is ordinary opaque, with `@captures(backend)` and `uses {S}`:
it exports no copying bound, has no automatic flush and keeps its callbacks' erased
authority.

```text
fn store.local_backend() -> store.Backend[Dir] uses {}
fn store.open_backend[S](take state: S, take backend: store.Backend[S], config: store.StoreConfig) -> Result[store.Store[S], store.StoreError] uses {state, backend}
fn store.into_state[S](take store: store.Store[S]) -> Result[S, store.StoreError] uses {store}
```

`open_backend` runs the same recovery, fencing and group-commit protocol as `open`.
An error from conditional creation is resolved by an uncached whole-object read-back:
exact requested bytes prove success, different bytes fence, and a failed read leaves
the store closed with the storage error. No blind create retry occurs. `into_state`
flushes like `close`, then returns the owned backend state for lifecycle management and
simulation observations. S3 is the pure Hirsel package `s3@1`, not a std dependency;
its backend and its deterministic simulator implement this dictionary. Cache and
transport policy live in the backend, and cache size zero disables caching.


## Compressed data

`std@1/flate` decodes whole buffers of DEFLATE (RFC 1951), zlib (RFC 1950) and
gzip (RFC 1952) data, ported from Go's `compress/flate`, `compress/zlib` and
`compress/gzip` readers with their test vectors:

```text
FlateError = Truncated | Corrupt(u64) | Header | Checksum | TooLarge | TrailingData(u64)
fn flate.inflate(data: Bytes, limit: u64) -> Result[unique Bytes, FlateError] uses {}
fn flate.zlib_decode(data: Bytes, limit: u64) -> Result[unique Bytes, FlateError] uses {}
fn flate.gzip_decode(data: Bytes, limit: u64) -> Result[unique Bytes, FlateError] uses {}
```

Output above `limit` bytes is `TooLarge()`, detected before the excess is written.
Input that ends inside a block, header or trailer is `Truncated()`. `Corrupt(n)`
covers reserved block types, stored lengths whose complement differs, code lengths
past 286 literal or 30 distance codes, repeats before the first length or past the
table, incomplete or oversubscribed codes (a single one-bit code is accepted, as
in zlib), reserved symbols 286, 287, 30 and 31, and distances before the start of
the stream's output; `n` is the number of input bytes holding the consumed bits.
Empty codes are accepted and corrupt only when used.

`inflate` and `zlib_decode` require the stream to end at the end of `data`; later
bytes are `TrailingData(offset)` (Go ignores them). `zlib_decode` accepts only
method 8 with a window of at most 32 KiB and a valid header check; a preset
dictionary is `Header()`, and an Adler-32 mismatch is `Checksum()`.
`gzip_decode` reads one or more concatenated members (Go's multistream default),
concatenating their output under one `limit`; a back-reference cannot reach into
an earlier member. Bytes after a member must start another member: fewer than 10
are `Truncated()`, and a wrong magic number, a method other than 8, a reserved flag
bit (RFC 1952 section 2.3.1.2; Go ignores them) or a header CRC mismatch is
`Header()`. Extra fields, names and comments are skipped. A CRC-32 or size
mismatch is `Checksum()`. No partial output is returned.

A `Decoder` decodes the same formats from a buffered source with bounded memory:

```text
ReadError[E] = Input(bufio.ReadError[E]) | Data(FlateError)
fn flate.decoder(format: Format) -> unique Decoder uses {}
@preserves_unique(out) fn flate.read[S,E](mut d: Decoder, mut r: bufio.Reader, mut source: S, refill: fn(mut S, mut List[u8], u64) -> Result[u64,E], mut out: List[u8], max: u64) -> Result[u64,flate.ReadError[E]]
@preserves_unique(out) fn flate.file_read(dir: Dir, mut f: File of dir, mut d: Decoder, mut r: bufio.Reader, mut out: List[u8], max: u64) -> Result[u64,flate.ReadError[FileError]]
```

`read` appends at most `max` bytes to `out` and returns how many; a short count is not
the end. It returns zero once the stream has ended: after the trailer for raw and zlib
data, whose following bytes stay unread in `r`, and for gzip at the end of the source
after a member. It pulls input through `bufio.read_more` only when the buffered bytes
cannot complete the unit it is decoding (a header field, block header, code table,
symbol or trailer), and consumes from `r` only the whole bytes it has decoded. The
decoder keeps one block's codes and at most 64 KiB of output history; `r` holds at most
one refill beyond the unit in progress. A source that ends inside the stream is
`Data(Truncated())`, `Corrupt(n)` counts the stream's input from its first byte, and the
other errors are those of the whole-buffer decoders, which share this decoder.
Decompression output is bounded only by what the caller asks for: a whole-buffer
`limit`, or the total of the `max` arguments.

Encoding ports zlib 1.3's `deflate.c` and `trees.c` at a 32 KiB window, memory level 8
and the default strategy, so every level writes exactly zlib's bytes:

```text
Format = Raw | Zlib | Gzip
fn flate.deflate(data: Bytes, level: u64) -> unique Bytes uses {}
fn flate.zlib_encode(data: Bytes, level: u64) -> unique Bytes uses {}
fn flate.gzip_encode(data: Bytes, level: u64) -> unique Bytes uses {}
fn flate.encoder(format: Format, level: u64) -> unique Encoder uses {}
@preserves_unique(out) fn flate.encode(mut e: Encoder, data: Bytes, mut out: List[u8]) -> () uses {}
@preserves_unique(out) fn flate.finish(mut e: Encoder, mut out: List[u8]) -> () uses {}
```

`level` runs from 0 to 9; a larger level fails MEM-14 at the call. Levels 1 to 3 use
zlib's greedy matcher and 4 to 9 its lazy one, with zlib's good, lazy, nice and chain
lengths; a block ends after 16383 symbols or at the end of input and is written stored,
fixed or dynamic, whichever is smallest. Level 0 writes stored blocks of 65535 bytes
and a final block holding the rest, possibly empty, as one zlib `deflate` call with
ample output does. The output for a level depends only on the input bytes: an
`Encoder` fed through any sequence of `encode` calls appends, by `finish`, the bytes
the one-call form returns. `encode` appends the output its input completes and keeps
at most a 64 KiB window, the hash chains and one block's symbols; `finish` appends the
rest and the trailer and leaves the encoder ready for a new stream of the same format
and level. The zlib header carries zlib's level flags and an Adler-32 trailer; the gzip
header has no name, comment or time, zlib's extra flags (2 at level 9, 4 below level 2)
and operating system 255 (unknown), with a CRC-32 and the input size modulo 2^32 after
the stream.

## Zstandard decoding

`std@1/zstd` decodes RFC 8878 frames in ordinary Hirsel source, adapted from
Zig's `std.compress.zstd`. It accepts raw, RLE and compressed blocks, FSE and
Huffman literal streams, repeat tables and offsets, and concatenated and
skippable frames. Every frame checksum is verified using XXH64.

```text
ZstdError = Truncated | BadMagic | ReservedBit | Corrupt | BlockOversize
  | Checksum | ContentSize | WindowTooLarge(u64) | OutputTooLarge
  | DictionaryRequired(u64) | DictionaryMismatch | InvalidDictionary
fn zstd.decode(data: Bytes, max_output: u64, max_window: u64) -> Result[unique Bytes, ZstdError] uses {}
fn zstd.dictionary(data: Bytes) -> Result[Dictionary, ZstdError] in data uses {}
fn zstd.decode_with_dictionary(data: Bytes, dict: Dictionary, max_output: u64, max_window: u64) -> Result[unique Bytes, ZstdError] uses {}
```

Both limits are explicit caller policy. `max_output` bounds the total output
across all frames; `OutputTooLarge()` is returned before exceeding it.
`max_window` bounds the history requirement declared by each frame, including
the content size of a single-segment frame. `WindowTooLarge(required)` refuses
an oversize header before allocating output or entropy tables for that frame.
No output buffer is allocated from an unchecked size. The whole-buffer decoder
keeps its bounded output as history, at most one block's literals (128 KiB),
and format-bounded entropy tables; it does not allocate a declared window.
Input and dictionary content are borrowed. No partial output is returned.

`dictionary` recognizes formatted dictionaries by magic `0xec30a437`, validates
their Huffman/FSE tables and nonzero repeat offsets within their content, and
otherwise accepts at least eight bytes as raw content. Structural truncation and
malformed tables return `Truncated()` or `Corrupt()`; invalid initial repeat
offsets, dictionary ID zero, and raw content shorter than eight bytes are
`InvalidDictionary()`. The dictionary can be reused without mutation. Its entropy
tables, repeat offsets and history are reset independently at each
frame. A nonzero frame dictionary ID requires a matching formatted dictionary;
absence is `DictionaryRequired(id)`, and a different ID is
`DictionaryMismatch()`. ID zero permits a caller-selected dictionary, including
a raw-content prefix. A frame cannot refer to a preceding frame's output.

`Truncated()` means a required byte field or section is incomplete; malformed
bitstreams, entropy tables, repeat modes and match distances are `Corrupt()`.
Reserved frame or sequence bits are `ReservedBit()`; the unused frame bit is
ignored as specified by RFC 8878. A block exceeding its frame's maximum is
`BlockOversize()`. Declared content-size mismatches are `ContentSize()` and
checksum mismatches are `Checksum()`. Empty input is `Truncated()`; a stream
containing only skippable frames succeeds with empty output. Trailing bytes
must form another complete frame; a non-Zstandard magic is `BadMagic()`.

## Tar archives

`std@1/tar` reads and writes tar archives: V7, USTAR and POSIX.1-2001 PAX headers on
read and write, GNU and STAR headers and GNU long names on read. It is a port of Go
`archive/tar` without sparse files.

```text
Kind = Regular | Link | Symlink | Char | Block | Directory | Fifo | Global | Other(u8)
Header { name: String, kind: Kind, linkname: String, size: u64, mode: u64, uid: u64, gid: u64, uname: String, gname: String, modified: i64, devmajor: u64, devminor: u64, records: Map[String,String] }
TarError = Header(u64) | Encoding(u64) | Unsupported(u64) | Truncated
ReadError[E] = Input(bufio.ReadError[E]) | Data(flate.FlateError) | Archive(TarError)
WriteError = Field | TooLong | Short | Closed
ExtractError[E] = Read(ReadError[E]) | Insecure(String) | Unsupported(String) | TooLarge | File(FileError)
fn tar.reader(compression: Option[flate.Format]) -> unique Reader uses {}
fn tar.writer() -> unique Writer uses {}
fn tar.header(take name: String, kind: Kind, size: u64) -> Header uses {}
fn tar.next[S,E](mut t: Reader, mut r: bufio.Reader, mut source: S, refill: fn(mut S, mut List[u8], u64) -> Result[u64,E]) -> Result[Option[Header],tar.ReadError[E]]
@preserves_unique(out) fn tar.read[S,E](mut t: Reader, mut r: bufio.Reader, mut source: S, refill: fn(mut S, mut List[u8], u64) -> Result[u64,E], mut out: List[u8], max: u64) -> Result[u64,tar.ReadError[E]]
fn tar.file_next(dir: Dir, mut f: File of dir, mut t: Reader, mut r: bufio.Reader) -> Result[Option[Header],tar.ReadError[FileError]]
@preserves_unique(out) fn tar.file_read(dir: Dir, mut f: File of dir, mut t: Reader, mut r: bufio.Reader, mut out: List[u8], max: u64) -> Result[u64,tar.ReadError[FileError]]
@preserves_unique(out) fn tar.write_header(mut w: Writer, h: Header, mut out: List[u8]) -> Result[(),WriteError]
@preserves_unique(out) fn tar.write(mut w: Writer, data: Bytes, mut out: List[u8]) -> Result[(),WriteError]
@preserves_unique(out) fn tar.close(mut w: Writer, mut out: List[u8]) -> Result[(),WriteError]
fn tar.extract[S,E](into: Dir, mut t: Reader, mut r: bufio.Reader, mut source: S, refill: fn(mut S, mut List[u8], u64) -> Result[u64,E], limit: u64) -> Result[u64,tar.ExtractError[E]]
fn tar.file_extract(from: Dir, mut f: File of from, into: Dir, mut t: Reader, limit: u64) -> Result[u64,tar.ExtractError[FileError]]
```

**Reading.** A `Reader` made with `Some(format)` decodes the archive through a
`flate.Decoder` of that format as it reads. `next` skips the rest of the current entry
and its padding and returns the next entry's header. It returns `None` after two zero
blocks, after one zero block followed by the end of the input, or at the end of the
input on a block boundary; after that every `next` returns `None`. `read` appends at
most `max` bytes of the current entry's data and returns how many, and zero at its end.
Link, symlink, device, directory and FIFO entries have no data, whatever their `size`.
The reader buffers the source in pieces of at most 32 KiB and holds at most one PAX or
GNU long-name payload (1 MiB at most) beyond that, so memory is bounded whatever the
archive. Bytes after the end of the archive may already have been read from the source.

The header block's checksum must match as an unsigned or a signed byte sum, and
numeric fields are octal or base-256. A negative base-256 field other than the
modification time is a `Header` error. A NUL type flag is `Regular`, or `Directory`
when the name ends in `/`. A USTAR or PAX header's 155-byte prefix and a STAR header's
131-byte prefix are joined to the name with `/`. A GNU header whose time fields do not
parse has its USTAR prefix applied, as Go reads archives written by Go 1.7. A GNU `L`
or `K` entry sets the next entry's name or link name, cut at its first NUL. A PAX `x`
header's records apply to the next entry: `path`, `linkpath`, `uname`, `gname`, `uid`,
`gid` and `size` replace those fields, and `mtime` replaces `modified` with its whole
seconds, rounded toward negative infinity. An empty record value keeps the header's
field. Only the last of several `L`, `K` or `x` headers applies. A PAX `g` header is
returned as a `Global` entry whose name is its `path` record or its header name, with
size 0. `records` holds every record that applied, in archive order.

`Header(n)` names the offset of the header block (or of the meta header) that is
malformed. That covers a bad checksum or number, a zero block followed by a non-zero
one, a malformed PAX record, a NUL in a key or in a `path`, `linkpath`, `uname` or
`gname` value, a non-decimal number or time, and a payload over 1 MiB. `Encoding(n)` is
a name or link name that is not UTF-8 after the replacements above, or a user or group
name that is not UTF-8.
`Unsupported(n)` is a GNU sparse entry (type `S` or GNU sparse PAX records).
`Truncated` is input that ends inside a header or an entry, and `Data` a compressed
stream that does not decode.

**Writing.** `write_header` completes the previous entry (`Short` when its data is
incomplete) by padding it to a block, then appends the header. It writes USTAR when
every field fits, a name over 100 bytes split at a `/` into prefix and name when it
can be. Otherwise it writes a PAX `x` header first, named `PaxHeaders.0/` beside the
entry, with records for the fields that do not fit: a name or link name that is long or
not ASCII, a user or group name over 32 bytes or not ASCII, a uid or gid of 2^21 or
more, a size of 8 GiB or more, and a negative or 2^33-or-later time. Records sorted by
key bytes follow Go's `"<length> <key>=<value>\n"` form. When a PAX header is written,
the USTAR fields that do not fit are cut or zeroed. `records` passed in are written
unless they would conflict. A basic record (`path`, `linkpath`, `uname`, `gname`,
`uid`, `gid`, `size`, `mtime`) is kept only when it equals the field. `atime`, `ctime`
and `GNU.sparse.*` are dropped, and any record makes the entry PAX. A `Global` header
writes a `g` header of all its records, named `GlobalHead.0.0` when unnamed, with no
entry after it. `Field` refuses a header that cannot be encoded: a mode or device
number of 2^21 or more, a NUL in the name, a record key that is empty or holds `=` or
NUL, a type flag `x`, `L` or `K`, a regular, device or FIFO entry whose name ends in
`/`, or records over 1 MiB. `write` appends data to the entry, and data past its size is
`TooLong` with nothing appended. `close` completes the last entry and appends two zero
blocks; later calls are `Closed`. Output matches Go's `tar.Writer` for these headers.

**Extraction.** `extract` reads every entry and writes directories and regular files
into `into`, creating missing parent directories and replacing existing files; it
returns the number of entries written. A name is refused as `Insecure(name)`, before
anything is written for it, when it is empty, absolute, holds a `..` component, a NUL
or a backslash, or has no component but `.`; leading `./` components are dropped. Every
existing component of the destination is examined without following symlinks: a
symlink there, including at the final component, is `Insecure`, and a non-directory
parent is `File(NotDirectory())`. Extraction therefore never writes through a symlink,
even one inside the grant, on top of `Dir`'s own confinement. Link, symlink, device,
FIFO and other entries are `Unsupported(name)`; `Global` entries are skipped.
`limit` bounds the total of regular files' declared sizes: an entry that would take it
past `limit` is `TooLarge` before any of its data is written. This is the expansion
limit for decompression bombs. Permissions, owners and times are not applied. The
first refusal stops extraction with the entries before it in place.

## X.509 certificate fields

```text
x509.X509Error   = Malformed(u64)
x509.Certificate = { subject: String, names: List[String], uris: List[String] }
fn x509.certificates(chain: Bytes) -> Result[List[Bytes], x509.X509Error] uses {}
fn x509.parse(der: Bytes) -> Result[x509.Certificate, x509.X509Error] uses {}
```

`std@1/x509` reads the identity fields of DER X.509 certificates (RFC 5280 section 4.1),
such as the client chain [`tls.peer`](core-library.md#network) returns. It verifies
nothing: signatures, validity periods and extensions other than the subject alternative
names are not checked, so a program trusts only fields of a chain a handshake verified.

`certificates` splits a chain into its certificates, in order: the input is DER
certificates concatenated, each a SEQUENCE that delimits itself, and the empty chain has
none. `parse` reads exactly one certificate, the whole input. `subject` is the subject
Name as an RFC 4514 string: the relative distinguished names last to first, separated by
`,`, and the attributes of a multivalued one in their DER order, separated by `+`. The
types `CN`, `L`, `ST`, `O`, `OU`, `C`, `STREET`, `DC` and `UID` use those names and every
other type its dotted-decimal OID. A UTF8String, PrintableString or IA5String value of a
named type is its text with `"`, `+`, `,`, `;`, `<`, `>` and `\` escaped by a backslash, as
are a leading space or `#` and a trailing space, and NUL written `\00`; any other value, a
value that is not UTF-8 and every value of a dotted-decimal type is `#` and the lowercase
hex of its whole DER element. `names` holds the subjectAltName extension's DNS names and IP
addresses in certificate order, an address written as `std@1/net.ip_to_string` writes it,
and `uris` its URIs, such as SPIFFE IDs; other name forms are skipped, and a certificate
without the extension has neither.

DER elements use definite lengths of at most four octets. An element that overruns its
enclosing one, a tag other than the one RFC 5280 places there, bytes after a certificate
in `parse`, a DNS name or URI that is not ASCII, an address of neither 4 nor 16 octets and
a malformed OID are `Malformed(at)`, with `at` the byte offset of the element that fails.
Both functions are pure ordinary source; outputs are owned.

## Zip archives

`std@1/zip` reads and writes zip archives (PKWARE APPNOTE 6.3) with stored and deflated
entries, data descriptors and ZIP64. It is a port of Go `archive/zip` without
encryption, disk spanning or custom compressors.

```text
Method = Stored | Deflate | Other(u64)
Entry { name: String, comment: String, method: Method, flags: u64, creator: u64, external: u64, modified: Option[i64], dos_time: u64, crc: u32, compressed: u64, size: u64, offset: u64 }
ZipError = Format(u64) | Encoding(u64) | Algorithm(String) | Checksum(String) | TooLarge | Data(flate.FlateError)
FileZipError = Storage(FileError) | Archive(ZipError)
WriteError = Field | Directory | Closed
ExtractError = Read(ZipError) | Insecure(String) | Unsupported(String) | File(FileError)
fn zip.entries(data: Bytes) -> Result[List[Entry],ZipError] uses {}
fn zip.archive_comment(data: Bytes) -> Result[unique String,ZipError] uses {}
fn zip.read(data: Bytes, e: Entry, limit: u64) -> Result[unique Bytes,ZipError] uses {}
fn zip.file_entries(dir: Dir, mut f: File of dir, size: u64) -> Result[List[Entry],FileZipError]
fn zip.file_read(dir: Dir, mut f: File of dir, e: Entry, limit: u64) -> Result[unique Bytes,FileZipError]
fn zip.extract(data: Bytes, into: Dir, limit: u64) -> Result[u64,ExtractError]
fn zip.file_extract(from: Dir, mut f: File of from, size: u64, into: Dir, limit: u64) -> Result[u64,ExtractError]
fn zip.writer() -> unique Writer uses {}
fn zip.entry(take name: String) -> Entry uses {}
@preserves_unique(out) fn zip.create(mut w: Writer, h: Entry, level: u64, mut out: List[u8]) -> Result[(),WriteError] uses {}
@preserves_unique(out) fn zip.create_raw(mut w: Writer, h: Entry, mut out: List[u8]) -> Result[(),WriteError] uses {}
@preserves_unique(out) fn zip.write(mut w: Writer, data: Bytes, mut out: List[u8]) -> Result[(),WriteError] uses {}
@preserves_unique(out) fn zip.close(mut w: Writer, comment: String, mut out: List[u8]) -> Result[(),WriteError] uses {}
fn zip.flushed(mut w: Writer, count: u64) -> () uses {}
```

**Reading.** `entries` finds the end-of-central-directory record: the last one, searched
in the final 1 KiB and then the final 65 KiB, whose comment fits in the data. When a
record count, size or offset is at its 16- or 32-bit maximum, it follows a ZIP64
locator just before the record (disk 0, one disk) to the ZIP64 end record. When the
directory's recorded offset does not hold a directory header, the base offset is the
gap between where the directory ends and where its size and offset say it should,
which allows a prefix (a self-extracting stub) and junk after the archive. Entries are
read in directory order, stopping at the first non-directory signature. The count must
match the record count's low 16 bits. A ZIP64 extra field supplies the sizes and
offset recorded as 0xFFFFFFFF, and an extended timestamp field (0x5455, modification
flag) sets `modified`. `offset` is absolute in `data`. A malformed record is
`Format(n)`. A name or comment that is not UTF-8 is `Encoding(n)` at its directory
header: no legacy code page is decoded.

`read` returns an entry's data. It is `TooLarge` before any decoding when `size`
exceeds `limit`, which bounds a decompression bomb by its declared size. Stored data
must be exactly `size` bytes. Deflated data is decoded with at most `size` bytes of
output, so data that decodes past `size`, or a stream that ends early or late, is a
`Format` error. The CRC-32 of the data must equal the entry's, unless that is zero
with no data descriptor. With a data descriptor (flag 8), the descriptor's CRC-32,
after an optional signature, must equal it too; otherwise the error is
`Checksum(name)`. A directory entry (a name ending in `/`) has no data; one with a
nonzero size is a `Format` error. `Other` methods are `Algorithm(name)`. `file_entries`
and `file_read` read only the last 65 KiB, the central directory and the one entry, by
seeking within the file.

**Writing.** A `Writer` appends to `out`. A caller that drains `out` reports the
drained count with `flushed`, so offsets stay right. `create` finishes the previous
entry and writes a local header with flag 8 and zero sizes. Then come the entry's data,
deflated at `level` through a `flate.Encoder` or stored, and on the next `create` or
`close` a data descriptor with signature, CRC-32 and sizes (64-bit when either size is
4 GiB or more). A name ending in `/` is a stored directory without a descriptor, and
writing data to it is `Directory`. `create_raw` writes the given CRC-32 and sizes into
the local header, with a ZIP64 extra field when a size exceeds 32 bits, and passes data
through unchanged. Flag 0x800 marks a name or comment holding a byte below 0x20, above
0x7D or a backslash. `modified` writes an extended timestamp extra field and the
MS-DOS date and time in UTC; otherwise `dos_time` is written as given. The version made
by keeps its high byte and records 2.0. `close` writes the central directory. It adds a
ZIP64 extra field for any size or offset at or above 0xFFFFFFFF (version 4.5), then
ZIP64 end and locator records when needed or when there are 65535 or more records, then
the end record with `comment`. Names, comments and extra fields over 65535 bytes, and
`Other` methods, are `Field`. Writes after `close` are `Closed`. The layout follows
Go's `zip.Writer`; deflated bytes are those of `std@1/flate`, which match zlib rather
than Go's compressor.

**Extraction.** `extract` writes every directory entry and file of the archive into
`into`, creating missing parent directories and replacing existing files; it returns
the number of entries written. Names are refused as `Insecure(name)` by the rules of
`tar.extract`, which also refuse a path through or onto an existing symlink. Entries
whose creator is Unix (high byte 3) and whose mode is a symlink, device, FIFO or socket
are `Unsupported(name)`. Each file's declared size counts against `limit` before it is
decoded, and decoding never exceeds that size, so at most `limit` bytes are written:
`Read(TooLarge())` is the expansion limit for zip bombs. One entry's data is held at a
time. Permissions and times are not applied. The first refusal stops extraction with
the entries before it in place.

## Cookies

`std@1/cookie` parses Cookie request headers and formats Set-Cookie response
headers, adapted from Go `net/http` with its tests (RFC 6265 section 4.1):

```text
SameSite    = Strict | Lax | None
SetCookie   = { name: String, value: String, path: Option[String], max_age: Option[u64],
                http_only: bool, secure: bool, same_site: Option[SameSite] }
CookieError = Blank | MissingEquals(u64) | InvalidName(u64) | InvalidValue(u64)
            | InvalidPath(u64) | InsecureSameSiteNone
fn cookie.new(name: String, value: String) -> SetCookie uses {}
fn cookie.parse(header: String) -> Result[unique List[(unique String, unique String)], CookieError] uses {}
fn cookie.format(c: SetCookie) -> Result[unique String, CookieError] uses {}
```

`parse` follows Go `ParseCookie`: it splits at every `;`, trims space, tab, CR and
LF, and returns the pairs in order with repeated names kept. Every pair needs `=`
(`MissingEquals`, so a trailing `;` is an error), a token name (`InvalidName`) and a
value of cookie-octets, space and comma (`InvalidValue`); one pair of enclosing
double quotes is removed. A blank header is `Blank()`. Offsets are those of the
trimmed pair, or of its value. Errors return no pairs.

`new` gives a cookie with no attributes; `format` writes `name=value` and then
`Path`, `Max-Age`, `HttpOnly`, `Secure` and `SameSite` in Go's order, double-quoting
a value that contains a space or comma. Where Go sanitizes or drops input, `format`
fails: an empty or non-token name is `InvalidName`, a value byte outside Go's
cookie value bytes is `InvalidValue`, and an empty path or a control character or
`;` in it is `InvalidPath`, each at the offending byte offset within that field.
`Max-Age=0` asks the client to delete the cookie. `SameSite=None` without `Secure`
is `InsecureSameSiteNone()`, since clients reject it (RFC 6265bis section 4.1.2.7).
Domain, Expires and Partitioned attributes, Set-Cookie parsing and cookie jars are
not provided.

## Arbitrary-precision integers

`bigint` is exact integer arithmetic of any size, in ordinary std source:

```text
pub opaque type bigint.BigInt: copyable + data + key + inert + noalloc_drop uses {}
fn bigint.zero() -> bigint.BigInt uses {}
fn bigint.from_T(value: T) -> bigint.BigInt uses {}
fn bigint.to_T(value: bigint.BigInt) -> Option[T] uses {}
fn bigint.is_zero(value: bigint.BigInt) -> bool uses {}
fn bigint.is_negative(value: bigint.BigInt) -> bool uses {}
fn bigint.signum(value: bigint.BigInt) -> i32 uses {}
fn bigint.compare(a: bigint.BigInt, b: bigint.BigInt) -> i32 uses {}
fn bigint.neg(value: bigint.BigInt) -> bigint.BigInt uses {}
fn bigint.abs(value: bigint.BigInt) -> bigint.BigInt uses {}
fn bigint.add(a: bigint.BigInt, b: bigint.BigInt) -> bigint.BigInt uses {}
fn bigint.sub(a: bigint.BigInt, b: bigint.BigInt) -> bigint.BigInt uses {}
fn bigint.mul(a: bigint.BigInt, b: bigint.BigInt) -> bigint.BigInt uses {}
fn bigint.div_rem(a: bigint.BigInt, b: bigint.BigInt) -> (bigint.BigInt, bigint.BigInt) uses {}
fn bigint.div_mod_floor(a: bigint.BigInt, b: bigint.BigInt) -> (bigint.BigInt, bigint.BigInt) uses {}
fn bigint.pow(base: bigint.BigInt, exponent: u64) -> bigint.BigInt uses {}
fn bigint.gcd(a: bigint.BigInt, b: bigint.BigInt) -> bigint.BigInt uses {}
fn bigint.bit_length(value: bigint.BigInt) -> u64 uses {}
fn bigint.shl(value: bigint.BigInt, bits: u64) -> bigint.BigInt uses {}
fn bigint.shr(value: bigint.BigInt, bits: u64) -> bigint.BigInt uses {}
fn bigint.bit_and(a: bigint.BigInt, b: bigint.BigInt) -> bigint.BigInt uses {}
fn bigint.bit_or(a: bigint.BigInt, b: bigint.BigInt) -> bigint.BigInt uses {}
fn bigint.bit_xor(a: bigint.BigInt, b: bigint.BigInt) -> bigint.BigInt uses {}
fn bigint.bit_not(value: bigint.BigInt) -> bigint.BigInt uses {}
fn bigint.parse(text: String) -> Result[bigint.BigInt, scalar.ParseError] uses {}
fn bigint.parse_radix(text: String, radix: u8) -> Result[bigint.BigInt, scalar.ParseError] uses {}
@preserves_unique(out) fn bigint.format_into(value: bigint.BigInt, mut out: List[u8]) -> () uses {}
fn bigint.to_string(value: bigint.BigInt) -> unique String uses {}
fn bigint.format_radix(value: bigint.BigInt, radix: u8) -> Result[unique String, scalar.ParseError] uses {}
```

`T` ranges over `u8` to `u128` and `i8` to `i128`, one concrete function per width.
`from_T` is total; `to_T` returns `None` exactly when the value is outside `T`.
The representation is a sign and a little-endian magnitude of `u64` limbs with no
high zero limb, and zero is never negative, so `==` and hashing compare values and
`BigInt` is a map key. `compare` and `signum` return -1, 0 or 1.

`div_rem` truncates toward zero and gives the remainder the dividend's sign, as
integer `/` and `%` do; `div_mod_floor` rounds toward negative infinity and gives
the remainder the divisor's sign, as `int.div_floor` and `int.mod` do. A zero
divisor fails SEM-1 at std's check, as integer division by zero does; nothing else
fails except allocation. `pow(x, 0)` is 1, including for zero. `gcd` is nonnegative and
`gcd(0, 0)` is 0. `bit_length` counts the magnitude's bits, 0 for zero.
`shl` multiplies by a power of two; `shr` divides by one, rounding toward negative
infinity. The bitwise functions act on the infinite two's-complement value, so
`bit_not(x)` is `-x - 1` and `bit_and(-1, x)` is `x`.

`parse_radix` accepts the integer grammar of [scalars](#scalars): an optional
leading minus and one or more digits of radix 2 through 36 in either letter case.
Leading zeros and `-0` are accepted; a plus, whitespace, underscores, radix
prefixes, an empty digit string and an invalid radix are `Invalid`; no value is
`OutOfRange`. `parse` is radix 10. Output has a minus for negative values, no
leading zeros (zero is `0`), lowercase letters and no prefix; `format_radix` with
an invalid radix is `Invalid`. `BigInt` declares `@format(bigint.format_into)`, so
an f-string hole prints its decimal text and takes fill, alignment and width as a
string does.

Magnitudes multiply by schoolbook when the shorter operand has at most 32 limbs and
by Karatsuba splitting otherwise, cutting an operand at least twice as long as the
other into chunks of the shorter one's length (num-bigint's `mac3` structure and
threshold). Division by one limb divides 128 by 64 bits per
limb; longer divisors use Knuth's algorithm D with the normalized two-limb quotient
estimate and its add-back correction. Radix conversion moves one limb's worth of
digits per multiply or divide (Go's `natconv`). These are ordinary source over
`List[u64]` with `u128` intermediates; there is no core hook.

## Exact decimals

`decimal` holds the exact value `coefficient * 10^-scale`, for amounts such as money
that must not round implicitly:

```text
pub type decimal.RoundingMode = HalfEven | HalfUp | Down | Up | Floor | Ceiling
pub opaque type decimal.Decimal: copyable + data + key + inert + noalloc_drop uses {}
fn decimal.new(unscaled: bigint.BigInt, places: i32) -> decimal.Decimal uses {}
fn decimal.coefficient(value: decimal.Decimal) -> bigint.BigInt uses {}
fn decimal.scale(value: decimal.Decimal) -> i32 uses {}
fn decimal.is_zero(value: decimal.Decimal) -> bool uses {}
fn decimal.signum(value: decimal.Decimal) -> i32 uses {}
fn decimal.compare(a: decimal.Decimal, b: decimal.Decimal) -> i32 uses {}
fn decimal.neg(value: decimal.Decimal) -> decimal.Decimal uses {}
fn decimal.abs(value: decimal.Decimal) -> decimal.Decimal uses {}
fn decimal.add(a: decimal.Decimal, b: decimal.Decimal) -> decimal.Decimal uses {}
fn decimal.sub(a: decimal.Decimal, b: decimal.Decimal) -> decimal.Decimal uses {}
fn decimal.mul(a: decimal.Decimal, b: decimal.Decimal) -> decimal.Decimal uses {}
fn decimal.div(a: decimal.Decimal, b: decimal.Decimal, places: i32, mode: decimal.RoundingMode) -> decimal.Decimal uses {}
fn decimal.rescale(value: decimal.Decimal, places: i32, mode: decimal.RoundingMode) -> decimal.Decimal uses {}
fn decimal.normalize(value: decimal.Decimal) -> decimal.Decimal uses {}
fn decimal.parse(text: String) -> Result[decimal.Decimal, scalar.ParseError] uses {}
@preserves_unique(out) fn decimal.format_into(value: decimal.Decimal, mut out: List[u8]) -> () uses {}
fn decimal.to_string(value: decimal.Decimal) -> unique String uses {}
```

The scale is part of the value's representation: `==` and hashing compare
coefficient and scale, so `1.0` and `1.00` differ under `==`, while `compare`
orders by value and returns 0 for them. `add` and `sub` are exact at the larger
scale, and `mul` is exact at the sum of the scales; a scale sum outside `i32` fails
SEM-1. Nothing else rounds implicitly. `div` returns `a / b` at exactly `places`
fractional digits, rounded by `mode`, and a zero divisor fails SEM-1. `rescale`
sets the scale to `places`: exact when it grows (`6.1` at 2 places is `6.10`) and
rounded by `mode` when it shrinks; a negative `places` rounds to a multiple of a
power of ten. `normalize` removes trailing fractional zeros, never lowering a
nonnegative scale below 0, and gives zero scale 0.

`HalfEven` and `HalfUp` round to the nearest value at the target scale, breaking a
tie to an even last digit and away from zero respectively. `Down` rounds toward
zero, `Up` away from zero, `Floor` toward negative infinity and `Ceiling` toward
positive infinity. Ties are exact comparisons of the discarded remainder; nothing
is rounded twice.

`parse` accepts `-?[0-9]+(\.[0-9]+)?` and keeps every written fractional digit as
the scale: `1.50` has coefficient 150 and scale 2, and `-0.00` is zero at scale 2.
A plus, exponent, whitespace, grouping, leading or trailing point and empty digit
string are `Invalid`; more than `i32` fractional digits are `OutOfRange`. Text has
a minus for negative values, at least one integer digit, and exactly `scale`
fractional digits after a point when the scale is positive; a negative scale
appends that many zeros to a nonzero coefficient. Text never has an exponent, so
`to_string(parse(s)) == s` for every accepted `s` without redundant leading zeros
or a negative zero. That text is also a valid JSON number: `json.Number(decimal.to_string(d))`
writes a decimal, and `decimal.parse` reads a JSON number lexeme that has no
exponent. `Decimal` declares `@format(decimal.format_into)` with the same padding
rules as `BigInt`. Scale rules and rounding modes follow rust_decimal, without its
96-bit coefficient or 28-digit scale limits.

## HTML escaping (`std@1/html`)

`html.escape(text: String) -> unique String` escapes `&`, `<`, `>`, `"` and
`'` as `&amp;`, `&lt;`, `&gt;`, `&#34;` and `&#39;`, respectively. Other
UTF-8 bytes are preserved. Existing character references are escaped again.
These are [HTML character references](https://html.spec.whatwg.org/multipage/syntax.html#character-references),
for element text and single- or double-quoted attribute values.

`html.escape_into(mut out: List[u8], text: String)` appends the same escaped
bytes and preserves `out`'s uniqueness. `html.esc(text: String) -> html.Escaped`
returns an owned wrapper with an `@format` association, so
`f"<p>{html.esc(value)}</p>"` escapes the hole. Ordinary String holes keep
their ordinary formatting. Escape each untrusted hole individually.
This is HTML text/attribute escaping; URL, JavaScript, CSS and unquoted
attribute contexts need their own validation or encoding.

Observation views of recorded std-library effects are available through
`hirsel view` (see [Offline observation views](determinism.md#offline-observation-views)).
They are external tooling: they add no source API, ambient context or authority.
`log` entries and stderr rendering keep their existing representation. Structured
application metadata must still be passed explicitly; a tape cannot reconstruct
missing domain names, trace parents or original elapsed time.

## Shell words

`std@1/shlex` splits and quotes POSIX shell words, as Python `shlex` does with
`posix=True`:

```text
SplitError = UnclosedQuote(u64) | TrailingEscape(u64)
fn shlex.split(line: String) -> Result[unique List[String], SplitError] uses {}
fn shlex.split_comments(line: String) -> Result[unique List[String], SplitError] uses {}
fn shlex.quote(word: String) -> unique String uses {}
fn shlex.join(words: List[String]) -> unique String uses {}
```

Space, tab, CR and LF separate words. Single quotes keep everything up to the
next single quote; inside double quotes a backslash escapes only `"` and `\` and
is kept before any other character; elsewhere a backslash takes the next
character literally. Quoted and unquoted parts join into one word, so `a"b"'c'`
is `abc` and `""` is an empty word. `split_comments` also treats an unquoted,
unescaped `#` as the start of a comment through the end of its line, ending any
word in progress, as Python's `comments=True`. Nothing is expanded. An open
quote at the end is `UnclosedQuote` at the quote's byte offset; a backslash at
the end, quoted or not, is `TrailingEscape` at its offset; errors return no
words. `quote` leaves a nonempty word of ASCII letters, digits and `%+,-./:=@_`
alone and otherwise single-quotes it, writing each `'` as `'"'"'`; `join` quotes
each word and separates them with one space, and `split` of the result gives the
words back.

## UUIDs

`std@1/uuid` parses, formats and generates RFC 9562 UUIDs:

```text
Uuid      = { bits: u128 }
Variant   = Ncs | Rfc9562 | Microsoft | Future
UuidError = Length(u64) | Char(u64)
V7Error   = Clock(ClockError) | Random(RandomError) | BeforeEpoch
fn uuid.parse(spelling: String) -> Result[Uuid, UuidError] uses {}
fn uuid.to_string(id: Uuid) -> unique String uses {}
fn uuid.from_bytes(data: Bytes) -> Option[Uuid] uses {}
fn uuid.to_bytes(id: Uuid) -> unique Bytes uses {}
fn uuid.version(id: Uuid) -> u8 uses {}
fn uuid.variant(id: Uuid) -> Variant uses {}
fn uuid.unix_ms(id: Uuid) -> Option[u64] uses {}
fn uuid.v4_from(random: Bytes) -> Option[Uuid] uses {}
fn uuid.v7_from(millis: u64, random: Bytes) -> Option[Uuid] uses {}
fn uuid.v4(r: Random) -> Result[Uuid, RandomError]
fn uuid.v7(clock: WallClock, r: Random) -> Result[Uuid, V7Error]
```

`bits` holds the 16 bytes most significant first, so comparing `bits` orders
version 7 UUIDs by time. `parse` accepts, as Go google/uuid and uuid-rs, the
hyphenated form `xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx`, the same in braces or
after `urn:uuid:` in any case, and 32 bare hex digits, in either case; the byte
length selects the form, so any other length is `Length(n)` and the first
character that does not fit is `Char(offset)`. `to_string` and `@format` write
the lowercase hyphenated form. `version` is the top four bits of byte 6 and
`variant` the top bits of byte 8, whatever their values; `unix_ms` is the
48-bit timestamp of a version 7 UUID of the RFC 9562 variant.

`v4_from` stamps the version 4 and variant bits onto 16 bytes, keeping the other
122. `v7_from` puts a millisecond Unix timestamp below 2^48 in the top 48 bits
and 10 bytes below it, the version replacing the first byte's top four bits and
the variant the third byte's top two, as uuid-rs
`Builder::from_unix_timestamp_millis`; other lengths or a larger timestamp are
`None()`. `v4` draws 16 bytes from `Random` and `v7` reads the wall clock,
rounding its nanoseconds down to milliseconds, then draws 10 bytes; a reading
before 1970 is `BeforeEpoch()`. No state is kept between calls, so two version 7
UUIDs of one millisecond are unordered (RFC 9562 section 6.2 makes the counter
optional).

## Semantic versions

`std@1/semver` parses SemVer 2.0 versions and matches them against Cargo
requirements and npm ranges:

```text
Version     = { major: u64, minor: u64, patch: u64, pre: String, build: String }
Op          = Exact | Greater | GreaterEq | Less | LessEq | Tilde | Caret | Wildcard
Comparator  = { op: Op, major: u64, minor: Option[u64], patch: Option[u64], pre: String }
Req         = { sets: List[List[Comparator]] }
SemverError = Empty | Syntax(u64) | LeadingZero(u64) | Overflow(u64)
fn semver.parse(spelling: String) -> Result[Version, SemverError] uses {}
fn semver.compare(a: Version, b: Version) -> i32 uses {}
fn semver.compare_pre(a: String, b: String) -> i32 uses {}
fn semver.parse_cargo(spelling: String) -> Result[Req, SemverError] uses {}
fn semver.parse_npm(spelling: String) -> Result[Req, SemverError] uses {}
fn semver.matches(req: Req, v: Version) -> bool uses {}
fn semver.to_string(v: Version) -> unique String uses {}
fn semver.req_to_string(req: Req) -> unique String uses {}
```

`parse` is strict, as dtolnay/semver: `major.minor.patch` in u64 without
leading zeros, then an optional `-` prerelease of dot-separated nonempty
`[0-9A-Za-z-]` identifiers (numeric ones without leading zeros) and an optional
`+` build of the same identifiers (leading zeros allowed). There is no leading
`v` or whitespace. Errors carry byte offsets: `Syntax` at the unexpected
character or the end, `LeadingZero` and `Overflow` at the number's start;
`Empty()` is the empty string. `compare` is SemVer precedence: numbers, then
prereleases, where a release is greater than any prerelease and identifiers
compare in turn, numeric by value and below alphanumeric ones in ASCII order,
a longer list winning a tie. Build metadata has no precedence, so `compare` can
be 0 for versions `==` tells apart. `Version` formats through `@format`.

A version matches a `Req` when it matches every comparator of some set; an
empty set matches every release. Comparators match as dtolnay/semver's do, so a
missing minor or patch widens them (`>1` is `>=2.0.0`, `~1.2` is `>=1.2.0,
<1.3.0`, `^0.0` is `>=0.0.0, <0.1.0`). A version with a prerelease matches a set
only if one of its comparators has a prerelease and the same major, minor and
patch, so `^1.2.3` rejects `1.3.0-beta` and `>=1.2.3-alpha` accepts `1.2.3-beta`.

`parse_cargo` reads a Cargo requirement into one set: comparators separated by
commas, each `=`, `>`, `>=`, `<`, `<=`, `~`, `^` or nothing (caret) before a
version whose minor and patch may be absent or `*`, `x` or `X`; spaces may
surround operators and commas, build metadata is ignored, and a lone wildcard
must be the only comparator. `parse_npm` reads a node-semver range without the
loose and includePrerelease options: `||` separates sets, each a hyphen range
`a - b` or comparators separated by whitespace (`<`, `<=`, `>`, `>=`, `=` or
nothing before an x-range such as `1`, `1.x` or `*`, `~`, `~>` or `^`), with an
optional `v` before versions; as in node-semver, every `+` and build
identifiers are first deleted from a set's text wherever they stand. Each comparator desugars to Exact, GreaterEq,
Greater, Less and LessEq comparators of full versions exactly as node-semver's
`Range` does (`^1.2` is `>=1.2.0, <2.0.0-0`, `>=0.0.0` is dropped, an
unsatisfiable comparator `<0.0.0-0` stands for its whole set, and so on); numbers
are u64 rather than JavaScript's 2^53. `req_to_string` spells a `Req` as
dtolnay/semver displays one: comparators joined by `, `, `*` for an empty set,
sets joined by ` || `.

## Line diffs and patches

`std@1/diff` compares sequences of lines, prints unified diffs and applies them:

```text
Algorithm  = Myers | Patience
Edit       = Keep(u64, u64, u64) | Delete(u64, u64, u64) | Insert(u64, u64, u64)
Patched    = { text: String, rejected: List[u64] }
PatchError = Malformed(u64)
fn diff.split_lines(source: String) -> unique List[String] uses {}
fn diff.edits(old: List[String], new: List[String], algorithm: Algorithm) -> unique List[Edit] uses {}
fn diff.unified(old_name: String, old: String, new_name: String, new: String, algorithm: Algorithm, context: u64) -> unique String uses {}
fn diff.apply(source: String, patch: String) -> Result[Patched, PatchError] uses {}
```

`split_lines` keeps each line's LF; a last line without one stays unterminated,
so lines compare whole and a missing final newline is a change. An edit script
is a sequence of runs `(old index, new index, count)` covering both sides in
order; a replacement is a Delete then an Insert.

`Myers()` is a minimal diff: Myers' O(ND) algorithm with the linear-space middle
snake (Myers 1986, section 4b). As in GNU diff and git, lines with no
counterpart on the other side are changed outright, and a line matching at
least sqrt(lines) others is changed too when it sits among such lines; each
group of changed lines is then slid down past equal lines, merging groups it
meets, and back up to the last place it lines up with a change on the other
side. `Patience()` matches lines that occur exactly once on each side, takes
their longest increasing run (Go `internal/diff`'s Szymanski LCS) as anchors,
extends each anchor over equal neighbours and diffs the gaps the same way,
falling back to Myers in a gap with no unique shared line. The output equals
`git diff --no-indent-heuristic` with the same algorithm whenever git stays
exact (its Myers abandons minimality only past an edit cost of 256 per split);
Myers here is always minimal, in O((N + M) D) time and O(N + M) memory.

`unified` prints `--- old_name`, `+++ new_name` and hunks headed
`@@ -start,count +start,count @@`, omitting a count of one and numbering an empty
range by the line before it, with `context` lines around changes and changes at
most `2 * context` lines apart sharing a hunk, as git does. A line without LF is
followed by `\ No newline at end of file`. Equal texts give the empty string;
there is no function-name context after `@@`.

`apply` takes the hunks of one file's unified diff, skipping lines before the
first `@@`, and applies them as GNU `patch --fuzz=0 --forward`: a hunk's old
lines must match exactly, first at its stated place shifted by the previous
applied hunk's offset, then at the nearest place not before the previous applied
hunk, later before earlier. A hunk with less leading than trailing context that
starts at line one applies only at the start of the file, one with less trailing
context only at its end. Hunks that match nowhere are listed in `rejected` by
their zero-based index and the rest apply. A body line other than ` `, `-`, `+`,
`\` or an empty context line, a count mismatch or text after the last hunk is
`Malformed` with its one-based patch line number. There is no fuzz, no reversed
patch detection and no multi-file patch.

## Retry and repeat schedules

`std@1/schedule` describes the waits between operation calls. Its name covers
both retries after errors and repeats after successes. It adapts Effect Schedule,
AWS jitter, and the Go/Rust backoff libraries; [research and pinned source
vectors](../research/std-schedule.md) record the choices.

A `Schedule` is an opaque, copyable value. A driver deep-copies it before advancing,
so the same policy can be reused independently. Durations, bases, factors and
limits are `u64`; durations are nanoseconds. Growth saturates at `u64` maximum.

```hirsel
fn retry_request(
  clock: MonotonicClock,
  random: Random,
  op: fn() -> Result[String, NetError],
  should_retry: fn(NetError) -> bool
) -> Result[(String, u64), schedule.Failure[NetError]] {
  let policy = schedule.max_attempts(
    schedule.max_elapsed(
      schedule.capped(schedule.jittered(schedule.exponential(100000000, 2), Full()), 5000000000),
      30000000000
    ),
    6
  )
  schedule.retry(clock, random, policy, op, should_retry)
}
```

Constructors are `spaced(interval)`, `fixed(interval)`, `exponential(base,
factor)` and `fibonacci(base)`. Exponential starts at `base`; Fibonacci starts
`base, base, 2*base, 3*base`. Spaced waits a full interval after each operation.
Fixed targets successive grid points measured from driver start, skipping
missed ticks after slow work. When behind by more than a full interval it
allows one immediate recurrence, then resumes the grid; zero is immediate. Zero bases and a
zero factor are legal. No constructor draws randomness or reads time.

Modifiers take a schedule first. `jittered(s, Full())` draws uniformly from
`0..delay`; `Equal()` draws from `floor(delay/2)..delay`, inclusive integer
endpoints. Singleton ranges draw nothing. `Decorrelated()` takes the first
child delay as its stable base, initializes previous to base, and draws from
`base..saturating(3*previous)`. The child still advances for its stop condition.
`capped(s, max_delay)` limits emitted waits; when it surrounds decorrelated
jitter, the capped wait becomes that branch's next previous value, as in AWS.
Cap after jitter to bound actual sleeps; jitter after cap changes the sampling
range and decorrelated jitter can then exceed that inner cap.

`max_attempts(s, n)` permits at most `max(1,n)` total calls, including the
immediate first call. `max_elapsed(s, ns)` includes time inside operations and
allows further calls strictly before the elapsed limit. A wait reaching or
exceeding that limit is rejected without sleeping. It rechecks time after a
wait in case the host overshoots. It does not interrupt a running operation;
use `task.scope_until` for that deadline. Wrapping a composition gives a global
budget; wrapping its children gives independent branch budgets.

`union(a,b)` chooses the shorter live delay and continues while either branch
continues. `intersect(a,b)` chooses the longer delay and requires both branches.
Both evaluate left then right once per decision and keep independent state;
stopping is fused. Deterministic delay traces obey commutativity, associativity,
idempotence and absorption. A stopped schedule is union's identity and
intersect's absorbing element; forever-zero is intersect's identity. Independent
jitter draws in separate branches preserve deterministic replay, but need not
preserve these trace equalities. Sequential composition is omitted to keep the
initial-operation and budget rules small.

`retry[T,E](clock, random, schedule, op, should_retry)` calls `op()` immediately
and returns `Ok((value, attempts))` on success. On every error,
`should_retry(error)` decides whether to continue. Permanent errors and an
exhausted schedule return `Error(GiveUp(last_error, attempts))`.
`repeat[T,E](clock, random, schedule, op)` repeats successes, returning the last
value and total calls when the schedule stops, and gives up immediately on an
operation error. Callbacks may capture the operation's capabilities.

Both return `Failure[E]`: `GiveUp(E,u64)` or
`Interrupted(Option[E],u64,ScheduleError)`. An interrupted retry retains its last
operation error; repeat uses `None()`. A clock failure before the initial call
has zero attempts. `ScheduleError` distinguishes `Clock(ClockError)`,
`Random(random.SampleError)` and `Sleep(time.SleepError)`.

All time and entropy pass through the supplied `MonotonicClock` and `Random`.
Waits use `time.sleep`, hence `clock.sleep_until`: scope cancellation interrupts
the wait and returns `Sleep(Wait(Canceled(cause)))` without another call.
`next(clock, random, mut schedule, start)` exposes the same state machine to
custom drivers, returning `Result[Option[time.Duration], ScheduleError]`; pass
the instant from before the initial operation. `None()` stays terminal.

### Explicit telemetry data (`std@1/telemetry`)

`telemetry` is ordinary source and inert data. `Value` admits only Bool,
Signed(i64), Unsigned(u64), Text and Bytes. `Attribute` pairs a name and value;
`Metadata` carries an optional explicit `TraceContext` and attributes. Neither
context nor metadata grants authority, selects a current task/span, or supplies
implicit inputs. Existing `log.Entry`, `log.to_string` and `log.write` are unchanged.
Counted fields follow ordinary ownership and sendability rules; serialization
never makes a counted value sendable.

`check_metadata` rejects duplicate names and sorts attributes by UTF-8 bytes.
`enrich(base, fields)` checks each input set, replaces base names explicitly
present in fields, then checks/sorts the union. Limits are 32 attributes, 1–128
UTF-8 bytes per name, and at most 4096 bytes per Text/Bytes value. Scalar integer
values retain their full widths. Errors are payload-free `EventError` variants;
no rejected name or value is retained in the error.

`allow_fields(fields, allow)` drops unlisted attributes before validation and
capture. Applications must apply this or their own redaction policy before
`check_log`; free-form names/messages, propagated tracestate and allowed values
require the application's explicit policy too. No automatic baggage forwarding
or secret discovery is performed.

`LogRecord` contains a `log.Level`, nonempty name (at most 128 UTF-8 bytes),
message (at most 4096 bytes), and Metadata. `check_log` validates and copies the
canonical record into an opaque `CheckedLog`, rejecting a complete rendered
line above 16384 bytes, including its newline. The accepted payload is immutable;
`log_data` returns a separate plain-data projection, and `render` returns its
validated JSON line. The opaque wrapper has no derived codec.

The v1 line has ordered top-level keys `version`, `level`, `name`, `message`,
`fields`, `trace`. Fields are a sorted JSON object; bool/integers/text render as
JSON scalars and Bytes as `{"base64":"..."}` using RFC 4648 standard padded
base64. Trace is null or an object with ordered keys `trace_id`, `span_id`,
`sampled`, `remote`, `tracestate` (an ordered array of name/value pairs). IDs are
lowercase hex. No timestamp is invented. `write_stderr(sink, minimum, checked)`
filters before output, then uses existing `err.print`: it is fallible and retains
that operation's cancellation and scheduling semantics. It is not the production
Telemetry capture contract. Recorded stderr bytes remain exact replay inputs.

`TraceContext` contains nonzero 16-byte trace and 8-byte span IDs, sampled/remote
bits and ordered tracestate. `validate_trace` checks raw contexts;
`extract_trace(headers)` returns an explicit ContextError on missing/malformed
input. Header names are ASCII case insensitive. This API pins W3C Trace Context
Recommendation 2021-11-23 wire **00**: exactly 55 lowercase characters with fixed
separators, nonzero IDs, and two hex flag digits. Only the sampled flag is kept;
unknown bits never become a W3C random flag. Newer versions are rejected.
Duplicate traceparent or tracestate headers fail. Tracestate keys/values follow
the W3C grammar, with no duplicate keys, at most 32 entries and 512 wire bytes.
Optional whitespace surrounding list members is removed; empty members and
malformed values fail. An empty tracestate header fails; an absent one is valid.
Extraction bounds the entire input to 128 headers and 16384 combined name/value
bytes, including ignored headers.

`inject_trace(raw)` is fallible so invalid program-created contexts cannot emit
malformed headers. It validates, emits one lowercase v00 traceparent and, when
nonempty, one tracestate in supplied order. It omits remote and baggage.
Applications explicitly choose whether to start a new root after extraction
fails. Context remains usable after its originating span ends and carries no
output, deadline or task authority. `SpanKind` and `Status` provide inert domain
vocabulary; live spans and capture are defined separately.

## Typed tables

`Table` stores its bounded UTF-8 name in an inline byte array, so copied names
retain their sendability without counted storage.

`std@1/table` holds typed table names and performs row operations through a
`store.Tx`. `Table[K, V]` is opaque inert data holding a name; it grants no
access by itself. Every key requires `key + serializable`, and every value
requires `serializable`. A key schema containing `Map`, `Set` or `Simd` is
refused with STORE-6, including nested and inactive-arm occurrences.

```text
fn table.named[K: key + serializable, V: serializable](name: String) -> table.Table[K, V]
fn table.create[K: key + serializable, V: serializable](mut tx: store.Tx, t: table.Table[K, V]) -> Result[(), table.TxError]
fn table.get[K: key + serializable, V: serializable](tx: store.Tx, t: table.Table[K, V], key: K) -> Result[Option[V], table.TxError]
fn table.put[K: key + serializable, V: serializable](mut tx: store.Tx, t: table.Table[K, V], key: K, value: V) -> Result[(), table.TxError]
fn table.delete[K: key + serializable, V: serializable](mut tx: store.Tx, t: table.Table[K, V], key: K) -> Result[bool, table.TxError]
fn table.range[K: key + serializable, V: serializable](tx: store.Tx, t: table.Table[K, V], from: table.RangeBound[K], to: table.RangeBound[K], limit: u64, order: table.ScanOrder) -> Result[List[(K, V)], table.TxError]
fn table.history(tx: store.Tx, name: String) -> Result[List[table.SchemaEntry], table.TxError]
fn table.rename(mut tx: store.Tx, name: String, from: String, to: String) -> Result[(), table.TxError]
```

Creation is explicit. `get` returns fresh owned data; `delete` reports whether a
row existed. `RangeBound` is `Unbounded | Included(K) | Excluded(K)` and
`ScanOrder` is `Ascending | Descending`; scans return at most a positive `limit`
rows, with `Excluded(last key)` for paging. Names are 1–255 UTF-8 bytes with no
NUL and no `hirsel.` prefix.

Schemas use names and shape with package versions excluded. Record fields
match by name, so declaration reordering reads old values correctly. Other
name differences return `Schema(Renamed(from, to))`. Declare that spelling
change with `rename`: nominal paths omit versions (`package/module/Type`),
and field/variant names append `.name`. Renames append history and rewrite no
row. A changed type requires a migration; it is never coerced on a read.
`range_rows` defaults to 4096; a larger requested limit is `Limit(RangeRows)`.
Writes preflight the complete prospective write set against the store limits.

History entries expose the epoch, SHA-256 of both storage descriptors and the
change. Rows retain their epoch followed by the canonical value payload;
decoding projects the stored field order into the expected type and applies
all canonical codec validation.

`TxError` is `InvalidInput | ReadOnly | NoTable | TableExists | Schema(SchemaError)
| Migrating | Limit(store.TxLimit) | Storage(store.StoreError)`;
`SchemaError` is `Renamed(String, String) | KeyMismatch | ValueMismatch`.
These and the history records are serializable. An operation's refusal leaves
the transaction running with no changes from that operation.

`table.migrate` and the reserved `table.drop` spelling are not implemented yet.
The former needs pure Entry invocation and code identity support; the latter
conflicts with the current reserved `drop` keyword.

A table's first key layout remains physical until a key migration. When a
record-key declaration is reordered, lookups project back to that layout and
ranges sort the projected keys in the caller's order. That fallback reads the
table's rows into an ordinary in-memory ordered map; schemas whose key layout
already agrees use a bounded index scan.

## Scoped CSV stream consumption

Prior-art mapping and deviations are recorded in
[the scoped driver port notes](../../reports/stream-scoped-port.md).

`stream.csv_fold_scoped` consumes a source directly, using the incremental CSV
state machine. It does not construct an escaping cursor or retain a Scope.
`csv.fold_scoped` is the same operation in the parser namespace.

```text
csv.StreamConfig {
  csv: csv.Config, refill_bytes: u64, max_input_bytes: u64,
  max_records: u64, max_outputs: u64
}
csv.DriverError[E] = Source(E) | Callback(E) | Csv(csv.CsvError)
  | Canceled(CancelCause) | Limit | InvalidRead | InvalidConfig
fn stream.csv_fold_scoped[S, A, E](scope: Scope, take source: S,
  config: csv.StreamConfig, take initial: A,
  refill: fn(mut S, mut List[u8], u64) -> Result[u64, E],
  accept: fn(List[String]) -> Result[bool, E],
  step: fn(take A, take List[String]) -> Result[A, E])
  -> Result[A, csv.DriverError[E]] uses {scope, source, initial, refill, accept, step}
```

Refill bytes must be positive and at most 65536, and the CSV record limit must
be positive; otherwise InvalidConfig is returned. Delimiter validation returns
Csv(InvalidDelimiter). These validations precede all checks, pulls and callbacks.
The input, raw-record and accepted-output budgets may be zero.

The driver owns the source, reader and accumulator for the duration of the
call. Reaching `max_outputs` succeeds immediately, including zero, without
another check, row or refill. Otherwise it checks cancellation before **each
raw-record advance**, even for rejected or already-buffered rows, and before
each refill. At those boundaries cancellation wins over an exhausted budget.
The raw-record limit counts accepted and rejected records alike. Reaching that
limit without reaching the output target returns Limit before another record
advance, including an advance that might discover EOF. A needed refill with
no input budget returns Limit **before** invoking the callback; max=0 is never
passed as an EOF probe. Exact-budget input without observed EOF therefore
requires either a satisfied output target or more budget for the EOF probe.

Each refill receives an empty list and an allowance no larger than the minimum
of `refill_bytes`, remaining input bytes, and remaining raw record bytes plus
one. The extra byte permits a terminator at the record ceiling. It must append
exactly its returned count, at most that allowance; otherwise InvalidRead.
Zero means clean EOF and finalizes one unterminated record, if present.
Source errors, including HTTP truncation or a source's own nested cancellation,
remain Source(E). Syntax, width, record-size and UTF-8 failures remain Csv.
Only successful records reach accept; only accepted records reach step.
Accept and step failures are Callback(E). The first refill/parser/callback
error returns immediately, with no subsequent cancellation check replacing it.
This is the fixed precedence even if cancellation became pending inside that
callback. There is no retry or rollback of effects already performed.

The driver finishes each step before advancing again, so a slow/suspending
sink supplies sequential backpressure. Pass an owned `http.Request` as S and
`http.read_body` as the scoped refill for HTTP → CSV → first N → sink; configure
input and record limits independently. Completion or any error destroys the
owned source and parser before returning. Incomplete HTTP bodies are dropped
and aborted, and trailers are discarded; callers needing trailers retain the
manual body/trailers protocol. Already-read lookahead can include later rows.
There is no producer task, queue, unbounded row list or implicit decompression.

Between checks parser work consumes at most one bounded refill or one bounded
raw record (plus CRLF handling). Retention comprises a refill buffer of at most
`refill_bytes`, the bounded incomplete record's field/row storage, and one
output row; record metadata/allocator capacity, source internal buffers and the
accumulator/callback scratch must be counted separately. An arbitrary callback
can allocate beyond its requested allowance before InvalidRead is detected,
retain every row in A, or run forever; this API cannot bound or interrupt that
work. Such callbacks need their own checks and capacity policy. Limits bound
logical input work and parser retention, not process RSS.

The check sequence and limits are program semantics for record/replay; they
cannot be changed by an optimizer or provider setting. Children obtain their
own Scope through a nested `task.scope` using captured global Tasks.
