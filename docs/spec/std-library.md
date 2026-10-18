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

`ascii` accepts the `u32` scalars produced by string iteration and character
literals. Digits are `'0'`–`'9'`, letters are `'A'`–`'Z'` and `'a'`–`'z'`,
and alphanumeric is their union. Space means ASCII space or U+0009–U+000D,
including vertical tab and form feed. Predicates are false outside ASCII;
case conversion preserves every nonletter, including non-ASCII values.
`digit_value` returns `Some(0)` through `Some(9)` for decimal digits and `None()`
otherwise. These are pure, nonallocating algorithms adapted from Zig's ASCII module.

## The catalogue

These declarations are normative: the source package must declare exactly these
signatures. `N` keeps the ledger's meaning of an explicit `@noalloc` promise.

```text
N fn ascii.is_digit(c: u32) -> bool uses {}
N fn ascii.is_alpha(c: u32) -> bool uses {}
N fn ascii.is_alnum(c: u32) -> bool uses {}
N fn ascii.is_space(c: u32) -> bool uses {}
N fn ascii.is_upper(c: u32) -> bool uses {}
N fn ascii.is_lower(c: u32) -> bool uses {}
N fn ascii.to_upper(c: u32) -> u32 uses {}
N fn ascii.to_lower(c: u32) -> u32 uses {}
N fn ascii.digit_value(c: u32) -> Option[u32] uses {}
fn path.normalize(p: Path) -> Result[Path, path.PathError] uses {}
fn path.join(base: Path, child: Path) -> Result[Path, path.PathError] uses {}
fn path.parent(p: Path) -> Option[Path] uses {}
fn path.filename(p: Path) -> unique String uses {}
fn path.extension(p: Path) -> unique String uses {}
fn path.components(p: Path) -> unique List[String] uses {}
N fn duration.from_parts(seconds: i64, nanoseconds: i32) -> time.Duration uses {}
N fn duration.parse(text: String) -> Result[time.Duration, scalar.ParseError] uses {}
fn duration.format(value: time.Duration) -> unique String uses {}
N fn time.parse(text: String) -> Result[time.Timestamp, scalar.ParseError] uses {}
fn time.format(value: time.Timestamp) -> Result[unique String, time.TimeError] uses {}
N fn time.elapsed(clock: MonotonicClock, start: Instant of clock) -> Result[time.Duration, ClockError]
N fn time.between(clock: MonotonicClock, start: Instant of clock, end: Instant of clock) -> time.Duration
N fn time.compare(clock: MonotonicClock, a: Instant of clock, b: Instant of clock) -> i32
N fn time.deadline(clock: MonotonicClock, interval: time.Duration) -> Result[Instant of clock, time.SleepError]
N fn time.sleep(clock: MonotonicClock, interval: time.Duration) -> Result[(), time.SleepError]
fn bytes.replace(src: Bytes, old: Bytes, new: Bytes) -> unique Bytes in src
N fn map.same_entries[K: key, V: data](a: Map[K, V], b: Map[K, V]) -> bool
N fn unicode.graphemes(s: String) -> GraphemeView in s
fn json.parse(take text: String) -> Result[Value, JsonError] uses {}
fn json.stringify(value: Value) -> Result[unique String, JsonError] uses {}
@preserves_unique(builder) fn json.write(value: Value, mut builder: List[u8]) -> Result[(), JsonError] uses {}
N fn test.equal[T: data](t: Test, actual: T, expected: T) -> ()
N fn test.approx(t: Test, actual: f64, expected: f64, absolute: f64, relative: f64) -> ()
fn test.for_all[T](t: Test, gen: fn(Test) -> T, law: fn(T) -> bool uses {}) -> ()
fn test.expect_error[T, E](t: Test, take value: Result[T, E]) -> E
fn input.read_all(s: Stdin) -> Result[Bytes, InputError]
fn input.read_line(s: Stdin) -> Result[Option[String], InputError]
N fn prng.seed(value: u64) -> prng.Generator uses {}
N fn prng.next_u64(mut generator: prng.Generator) -> u64 uses {}
N fn prng.below(mut generator: prng.Generator, upper_exclusive: u64) -> Result[u64, prng.SampleError] uses {}
fn prng.fill(mut generator: prng.Generator, mut out: List[u8], count: u64) -> () uses {}
fn random.u64(r: Random) -> Result[u64, RandomError]
fn random.below(r: Random, upper_exclusive: u64) -> Result[u64, random.SampleError]
```

Modules beyond the catalogue (`list`, `string`, `text`, `net`, `uri`, `http_wire`, `http`, `bufio`
and the `format_into` and NFC functions below) define their contracts by their declared
signatures and the rules in this part.

## Paths

`path.PathError = Invalid(u64)` reports a byte offset. Components are separated by
slashes only, and empty components are ignored. `normalize` removes `.` components and
cancels `..` against an ordinary preceding component, preserves leading relative `..`,
rejects escaping above an absolute root and rejects NUL and backslash. An empty relative
result is `.`; an absolute empty component stack is `/`. `join` requires a relative child,
rejects NUL and backslash in both arguments and normalizes the combined path; an empty base
contributes no separator. `parent`, `filename` and `components` inspect the raw nonempty
slash components, ignoring trailing slashes, without normalizing. Empty and root paths have
no parent or file name; a single relative component has parent `.`, and a single absolute
component has parent `/`. `extension` is the last non-leading dot suffix; empty means both
an absent extension and an empty suffix (`a.`). These functions take no `Dir`, touch no
host, resolve no links and establish no confinement.

## Durations and timestamps

`time.Duration` holds signed `i128` nanoseconds. `duration.from_parts` widens signed
`i64` seconds and signed `i32` nanoseconds before summing, with no range imposed on the
fractional part. `duration.parse` accepts an optional sign, integer decimal seconds, an
optional dot followed by one to nine fraction digits, and a final `s`, with no whitespace.
Invalid grammar is `scalar.ParseError.Invalid`; a value not representable as signed
`i128` nanoseconds is `OutOfRange`. `duration.format` covers the whole signed `i128` range
and emits canonical seconds with a final `s` and trimmed fractional zeros; zero is `0s`.

`time.Timestamp` holds signed `i128` `unix_nanoseconds`; `TimeError` is `OutOfRange`.
`time.parse` accepts RFC 3339 with Gregorian years 0000–9999, a `T` or `t` separator, a `Z`,
`z` or numeric `±HH:MM` offset (hours 00–23, minutes 00–59), and an optional fraction of one
to nine digits, validating dates and 00–23/00–59/00–59 times. Space separators, a leap
second 60, a missing offset and trailing data are invalid. Numeric offsets normalize to UTC,
`-00:00` included; an offset may move the UTC result outside the formatting range.
`time.format` returns UTC with a four-digit year, uppercase `T` and `Z` and trimmed
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

## JSON

```text
Value     = Null() | Bool(bool) | Number(String) | Text(String)
          | Array(List[box Value]) | Object(Map[String, box Value])
JsonError = Syntax(u64) | DuplicateKey(u64) | Nesting(u64) | InvalidNumber()
```

`json.parse` takes a UTF-8 `String` and returns an owned tree, keeping validated decimal
number lexemes exactly. It accepts JSON whitespace only, validates escaped UTF-16 surrogate
pairs, rejects duplicate decoded object keys and preserves insertion order. `Syntax`,
`DuplicateKey` and parser `Nesting` payloads are zero-based byte offsets into the input; end
of input is its byte length, and `DuplicateKey` points at the repeated key's opening quote.
The constant `MAX_NESTING: u64 = 128` counts open arrays and objects, scalars using no
level; opening container 129 is `Nesting`. A partial tree is released through ordinary error
cleanup.

`json.stringify` borrows a `Value` and returns compact UTF-8 JSON; `json.write` borrows it
and appends to a mutable builder, keeping existing content and possibly leaving a written
prefix on error. Both validate every `Number` against
`-?(0|[1-9][0-9]*)(\.[0-9]+)?([eE][-+]?[0-9]+)?`, requiring complete consumption, and return
`InvalidNumber` otherwise. They preserve number spelling and object order, escape ASCII
controls, quotes and backslashes, and leave other scalars unescaped. Writing uses the same
nesting bound, returning `Nesting(128)` at the attempted container depth. This is library
policy, independent of the depth limits of [MEM-3](memory.md#mem-3-freeing), with no
reflection, automatic record conversion or authority.

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
`text.pad_from(start: u64, fill: u64, align: u64, width: u64, mut out: List[u8]) -> () uses {}`,
or `text.pad_number_from` with the same parameters for an integer or float, where
`start` is the builder's byte length before the append: they pad the text appended since
`start` to `width` Unicode scalars, left-aligned and right-aligned by default
respectively. Any other specified hole calls the value's module's
`format_into_spec(value, fill: u64, align: u64, width: u64, precision: u64,
flags: u64, kind: u64, mut out: List[u8]) -> () uses {}`; `String` uses
`text.format_into_spec`. These operations preserve the builder's uniqueness.
The parser supplies the options: fill is a Unicode scalar (default space), alignment
is 0 default, 1 left, 2 right or 3 center; flags are bit 1 plus, bit 2 alternate,
bit 4 sign-aware zero padding and bit 8 precision present; kind is 0 display or
the ASCII code of `x`, `X`, `b` or `o`. Width and precision are decimal u64 constants
or, written `name$`, the value of a `u64` name in scope.
Ordinary callers must supply these same valid option encodings.

- `{x:9}` sets a minimum width, right-aligned for numbers and left-aligned for
  strings and bool. `<`, `>` and `^` choose left, right and center alignment;
  odd center padding puts the extra scalar on the right. `*^9` chooses `*` as fill.
  Width and string precision count Unicode scalars, not UTF-8 bytes or graphemes.
- `{x:09}` pads numbers with zeros after their sign and radix prefix, overriding
  alignment and fill. `{x:+}` adds a plus for nonnegative numbers; a minus remains
  on negative values, including negative zero. `-` is the default sign policy.
- `{x:.2}` formats a float with exactly two fractional decimal digits, rounded
  from its exact binary value with ties to even, including carry into the integer
  part. Default float formatting retains its shortest round-trip representation
  and existing fixed/scientific thresholds. Both paths use exact decimal rounding.
  NaN and infinities keep their ordinary spelling; NaN has no sign. Strings and bool are truncated
  to at most the precision's scalar count before padding; integer precision is ignored.
- `{x:x}`, `{x:X}`, `{x:b}` and `{x:o}` format integers in lowercase hex,
  uppercase hex, binary and octal. Signed radix output uses the scalar's two's
  complement bit width. `#` adds `0x`, `0b` or `0o` (also `0x` for uppercase hex).
  Radix formats on other types are SEM-3. String and bool formatting ignores sign,
  alternate and zero flags; alternate display has no effect on numbers.

- `{x:w$}`, `{x:.p$}` and `{x:>w$.p$}` take the width or precision from the `u64` named
  `w` or `p`, exactly as the same constant would.
- A `@format` type's text takes fill, alignment and width as a string's does, counting
  Unicode scalars and left-aligned by default; precision, sign, `#`, `0` and radix
  formats on it are SEM-3.

An empty specification is default formatting. Unknown type characters, malformed
options, overflowing constants and positional (`1$`) or `*` counts are SEM-3.

## Scalars

`bool`, `u8` to `u128`, `i8` to `i128`, `f32` and `f64` each have a module exposing, with
`uses {}`:

- `to_string(value) -> unique String`;
- `format_into(value, mut out: List[u8]) -> ()`, which appends exactly the text `to_string`
  returns and fails only by allocation exhaustion;
- `format_into_spec(value, fill: u64, align: u64, width: u64, precision: u64,
  flags: u64, kind: u64, mut out: List[u8]) -> ()`, using the constant option
  encoding above;
- `@noalloc format[n: size](value, mut buffer: Inline[u8, n]) -> Result[u64, scalar.FormatError]`,
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
List sort share one sift kernel; reversing its comparator never negates an i32.
A fresh empty heap is unique. This surface provides no peek or indexed update.

## Testing helpers

`test.equal` checks structural equality of `data` values. `test.approx` requires
nonnegative tolerances, accepts exactly equal values, rejects unequal nonfinite values, and
otherwise passes when the difference is within the absolute tolerance or within the
relative tolerance of the larger magnitude. `test.for_all` runs exactly 100 sequential
generator and law calls using the run's Test grant, reporting each through
`test.check_case`, without shrinking. `test.expect_error` returns the error of an `Error`,
and destroys an unexpected `Ok` value before failing the test with `test.fail`.

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
words with four successive SplitMix64 outputs. `prng.next_u64` returns the next Xoshiro256++
output. `prng.below(g, upper)` returns `Error(EmptyRange())` for `upper == 0` without
advancing; otherwise it draws until Lemire's multiply-shift accepts a draw (rejecting the
low products below `2^64 mod upper`) and returns the high word of the accepted product, so
every value below `upper` is equally likely and modulo reduction is never used.
`prng.fill(g, out, count)` appends `count` bytes, each output little-endian, cutting the
last output short. The generator is a pure deterministic function of its seed: identical
on every engine and target, it holds no authority and is never cryptographically secure.
Programs that need secrets draw from their `Random` capability instead.

## Unicode

Grapheme boundaries and normalization use the
[Unicode 16.0.0](https://www.unicode.org/versions/Unicode16.0.0/) tables and default extended
grapheme clusters, without locale tailoring. `GraphemeView` and `GraphemeCursor` are ordinary
opaque types of `std@1/unicode`; `unicode.graphemes(s)` is a borrowed loop source yielding
each cluster as a `String` related to `s`. `unicode.normalize_nfc(s)` returns the canonical
composition of `s`; compatibility mappings never apply.

## Bytes, maps, lists and strings

`bytes.replace(src, old, new)` scans left to right, replaces nonoverlapping matches, and
fails SEM-3 for an empty pattern. `map.same_entries` compares two maps as sets of entries.

`std@1/list` provides `reverse`, `rotate` (left rotation by `amount`), `sort` (in place,
unstable, `O(n log n)`, by a noalloc comparator), `binary_search` (the first equal element
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
`parse_socket_addr` return `AddrParseError = Invalid()` on bad input; `format_ip` uses RFC
5952 compression for IPv6, and `format_socket_addr` brackets IPv6 addresses.
`validate_host` checks a host name, returning `HostError = NameTooLong | InvalidHostName`,
and `ends_in_number` is the numeric-host test of
[CAP-1](capabilities.md#cap-1-authority). Zone and flow identifiers are not represented.

`std@1/uri` parses, formats and resolves RFC 3986 references: `parse`, `format`,
`resolve`, `remove_dot_segments`, `percent_encode` and `percent_decode`, with components
kept as `Raw(String) | Encoded(String)` and `UriError = UnexpectedCharacter |
InvalidFormat | InvalidPort | InvalidHostName`. A reference with a syntactically valid scheme
keeps its absolute-parse error; only scheme-less references are parsed as relative, so
`resolve(base, "http://127.1")` is `InvalidHostName`. Hosts apply the numeric-host rule after
percent decoding.

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
fn bufio.read[S,E](mut r: Reader, mut source: S, refill: fn(mut S, mut List[u8], u64) -> Result[u64,E], mut out: List[u8], max: u64) -> Result[u64,ReadError[E]]
fn bufio.read_exact[S,E](mut r: Reader, mut source: S, refill: fn(mut S, mut List[u8], u64) -> Result[u64,E], mut out: List[u8], count: u64) -> Result[(),ReadError[E]]
fn bufio.read_until[S,E](mut r: Reader, mut source: S, refill: fn(mut S, mut List[u8], u64) -> Result[u64,E], delimiter: u8, mut out: List[u8], limit: u64) -> Result[u64,ReadError[E]]
fn bufio.read_line[S,E](mut r: Reader, mut source: S, refill: fn(mut S, mut List[u8], u64) -> Result[u64,E], limit: u64) -> Result[String,ReadError[E]]
fn bufio.stream_read_more(n: Net, mut s: Stream of n, mut r: Reader, max: u64) -> Result[u64,ReadError[NetError]]
fn bufio.stream_read(n: Net, mut s: Stream of n, mut r: Reader, mut out: List[u8], max: u64) -> Result[u64,ReadError[NetError]]
fn bufio.stream_read_exact(n: Net, mut s: Stream of n, mut r: Reader, mut out: List[u8], count: u64) -> Result[(),ReadError[NetError]]
fn bufio.stream_read_until(n: Net, mut s: Stream of n, mut r: Reader, delimiter: u8, mut out: List[u8], limit: u64) -> Result[u64,ReadError[NetError]]
fn bufio.stream_read_line(n: Net, mut s: Stream of n, mut r: Reader, limit: u64) -> Result[String,ReadError[NetError]]
fn bufio.file_read_more(d: Dir, mut f: File of d, mut r: Reader, max: u64) -> Result[u64,ReadError[FileError]]
fn bufio.file_read(d: Dir, mut f: File of d, mut r: Reader, mut out: List[u8], max: u64) -> Result[u64,ReadError[FileError]]
fn bufio.file_read_exact(d: Dir, mut f: File of d, mut r: Reader, mut out: List[u8], count: u64) -> Result[(),ReadError[FileError]]
fn bufio.file_read_until(d: Dir, mut f: File of d, mut r: Reader, delimiter: u8, mut out: List[u8], limit: u64) -> Result[u64,ReadError[FileError]]
fn bufio.file_read_line(d: Dir, mut f: File of d, mut r: Reader, limit: u64) -> Result[String,ReadError[FileError]]
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
loop to LF and decodes owned UTF-8, preserving the LF; EOF before any bytes yields `""`.
Invalid UTF-8 returns `InvalidUtf8` after consuming those bytes. All reads propagate source
errors as `Source`; operations taking an output list retain already appended bytes. The Stream adapters use
`core@1/net.read` with the same rules and preserve the stream's Net brand.
The File adapters mirror them using `core@1/file.read`, preserving `File of d`.
Discard buffered state before seeking or otherwise reading the underlying File directly.

A line read over an existing branded Stream needs only persistent buffer state:

```hirsel
fn line(n: Net, mut stream: Stream of n) -> Result[String, bufio.ReadError[NetError]] {
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

`std@1/http` is an HTTP/1.1 client over the core network operations:

```text
Method       = Get | Head | Post | Put | Delete | Patch | Options
Version      = Http10 | Http11
Header       = { name: String, value: String }
Config       = { max_redirects: u32, connect_timeout: Option[Duration],
                 read_timeout: Option[Duration], write_timeout: Option[Duration],
                 max_head_bytes: u64, max_headers: u64, user_agent: String,
                 roots: TlsRoots, status_as_error: bool }
ResponseHead = { version: Version, status: u16, reason: String, headers: List[Header],
                 content_length: Option[u64], chunked: bool, keep_alive: bool,
                 location: Option[String], content_type: Option[String] }
Response     = { status: u16, headers: List[Header], body: Bytes, url: String }
HttpError    = Net(NetError) | Url(uri.UriError) | Malformed(http_wire.HeadError)
             | Chunk | HeadTooLarge | BodyTooLarge | Truncated | TooManyRedirects
             | RedirectRequiresResend | RedirectLocationInvalid | UnsupportedScheme
             | UnsupportedTransferEncoding | Status(u16) | OutOfOrder

pub opaque resource type Request: noalloc_drop uses {Net}

fn default_config() -> Config uses {}
fn request(n: Net, config: Config, method: Method, url: String, headers: List[Header]) -> Result[Request, HttpError]
fn send_bodiless(mut r: Request) -> Result[(), HttpError]
fn send_body(mut r: Request, body: Bytes) -> Result[(), HttpError]
fn send_chunk(mut r: Request, data: Bytes) -> Result[(), HttpError]
fn end_body(mut r: Request, request_trailers: List[Header]) -> Result[(), HttpError]
fn receive_head(mut r: Request) -> Result[ResponseHead, HttpError]
fn read_body(mut r: Request, mut out: List[u8], max: u64) -> Result[u64, HttpError]
fn read_all(mut r: Request, limit: u64) -> Result[Bytes, HttpError]
fn trailers(r: Request) -> List[Header]
fn header(headers: List[Header], name: String) -> Option[String] uses {}
fn fetch(n: Net, config: Config, method: Method, url: String, headers: List[Header], body: Option[Bytes]) -> Result[Response, HttpError]
```

`default_config()` is 3 redirects, no timeouts, 64 KiB of head, 100 headers, user agent
`hirsel-std/1`, `WebPki()` roots and `status_as_error` true. A `Request` holds its `Net`, a
`Stream` branded by it, the parsed URL, the redirect budget, an input buffer and the
response state, so its operations take only `mut r` and its authority is `{Net}`; dropping
it drops the stream.

- `request` parses the URL, maps `http` and `https` to ports 80 and 443 (other schemes are
  `UnsupportedScheme()`), connects with the connect timeout, installs the read and write
  timeouts, and for `https` upgrades with `tls.connect` using the URL's host and the
  configured roots, so the timeouts also bound the handshake. Header names and values
  containing CR or LF, and names containing `:`, are `Malformed(...)`: this is the
  header-injection boundary for agent-supplied strings.
- `send_bodiless` and `send_body` write the request head (`METHOD target HTTP/1.1`, `host`,
  `user-agent`, `connection: close`, the caller's headers, and `content-length` for a
  body) and the body. `send_chunk` and `end_body` stream a `transfer-encoding: chunked`
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
`http_wire.framing`; no `accept-encoding` is sent. **Redirects** happen only for bodiless
requests or after a 303; a 303, and a 301 or 302 after a `POST`, become a bodiless `GET`;
a 307 or 308 after a body has been sent is `RedirectRequiresResend()`; more than
`max_redirects` is `TooManyRedirects()`. The `Location` is resolved against the current URL
with `uri.resolve`. `authorization`, `cookie` and `proxy-authorization` are kept only within
the exact canonical origin (scheme, host and port). A redirect target goes through the same
`Net`, so a target outside the grant fails with `Net(NotGranted())`, and an https-to-http
downgrade fails under a `ConnectTls` rule. There is no connection reuse: each request opens
its own connection and sends `connection: close`, while `keep_alive` is still reported.
There is no whole-request deadline, since a deadline needs a clock.

## Input

`input.read_all(s: Stdin) -> Result[Bytes, InputError]` reads to end of input.
`input.read_line(s: Stdin) -> Result[Option[String], InputError]` reads through
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

## HTTP server

The server helpers share the client's wire parser and error type:

```text
RequestLine = { method_start: u64, method_end: u64, target_start: u64, target_end: u64, version: u8 }
N fn http_wire.parse_request[k: size](buf: Bytes, mut line: RequestLine, mut headers: Inline[RawHeader, k], mut count: u64) -> Result[ParseStatus, HeadError] uses {}
ServerRequest = { method: String, target: String, version: Version, headers: List[Header], body: Bytes }
fn http.parse_server_request(buf: Bytes, max_head_bytes: u64, max_body_bytes: u64) -> Result[Option[(ServerRequest, u64)], HttpError] uses {}
fn http.receive_request(n: Net, mut s: Stream of n, max_head_bytes: u64, max_body_bytes: u64) -> Result[ServerRequest, HttpError]
fn http.respond(n: Net, mut s: Stream of n, status: u16, headers: List[Header], body: Bytes) -> Result[(), HttpError]
```

`parse_request` follows httparse's strict request-line delimiters, accepts HTTP/1.0 and
HTTP/1.1, and reuses `parse_headers`. Its initialized-header count and absolute consumed
head offset follow `parse_response`. `parse_server_request` returns an owned request and
the consumed head-plus-body byte count, or `None` for incomplete input. It accepts at most
16 headers, checks the caller's head and body byte limits, and reads a Content-Length body
(zero bytes when absent). Malformed heads and framing fail; transfer codings are
unsupported. Bytes after the declared body are excluded from the request.

`receive_request` reads until that request completes; EOF before completion is `Truncated`.
`respond` validates status and headers, writes the body with its exact Content-Length and
`Connection: close`, and rejects caller-supplied Content-Length, Transfer-Encoding and
Connection headers. The caller owns the stream, closes it after the response, and owns the
sequential listen/accept loop. A server maps malformed requests to a 400 response before
invoking application handling. No routing, concurrency or persistent-connection framework
is implied.

## Unicode scalar properties and simple case

The pure allocation-free `std@1/unicode` helpers use the same pinned Unicode
16.0.0 data as NFC and grapheme segmentation:

```text
fn unicode.is_whitespace(cp: u32) -> bool uses {}
fn unicode.is_letter(cp: u32) -> bool uses {}
fn unicode.is_digit(cp: u32) -> bool uses {}
fn unicode.simple_lower(cp: u32) -> u32 uses {}
fn unicode.simple_upper(cp: u32) -> u32 uses {}
```

Whitespace is the Unicode White_Space property, letters are general categories
L*, and digits are Nd. Invalid scalar values return false from predicates and
pass through unchanged from case functions. Unmapped scalars also pass through.
These helpers classify scalar numbers; they do not validate text. Simple case
maps one scalar to one scalar, without full string casing, casefold or locale
rules. Generated immutable ranges and mapping rows come from the vendored
UnicodeData.txt and PropList.txt; `stdlib/unicode16/generate.py --check` verifies
byte-identical NFC and property table regeneration.

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
Args through arg.count/text once, parse typed values separately, and choose how
to print help or errors. There is no registration, ambient Args access or exit.

## Explicit-sink structured logging

`std@1/log` defines `Level = Debug | Info | Warn | Error`, ordered in that
severity order, `Field { name: String, value: String }`, and
`Entry { level: Level, message: String, fields: List[Field], timestamp:
Option[time.Timestamp] }`.

```text
fn log.format(entry: Entry) -> String uses {}
fn log.write(sink: Stderr, minimum: Level, entry: Entry) -> Result[(), OutputError]
```

Format is pure source and emits one compact JSON object followed by exactly one
LF, with keys in this order: level (lowercase name), message, fields (ordered
objects with name/value strings), timestamp (integer Unix nanoseconds or null).
The field list preserves duplicates and reserved names without shadowing core
keys. Existing JSON escaping handles all strings, including embedded newlines.
Timestamp is caller-supplied; no wall clock is read. Write filters below minimum
before formatting, then calls err.print once with the complete record and
propagates its error. There is no global logger, implicit sink, hidden clock or
ambient level. Ordering/atomicity across tasks is not promised; callers needing
ordering serialize through their explicit task/channel facilities.

## Additional testing helpers

`test.expect_ok[T,E](t: Test, take value: Result[T,E]) -> T` returns an owned
Ok payload. An unexpected Error payload is destroyed before test.fail, matching
expect_error's cleanup rule. A failing payload finalizer therefore precedes the
assertion failure. `test.for_all_cases[T](t: Test, cases: u64, gen: fn(Test) -> T,
law: fn(T) -> bool uses {}) -> ()` runs exactly the requested sequential count,
reporting deterministic zero-based case indices through check_case. Zero calls
neither callback. Existing for_all selects 100 over the same kernel. Each generated
value is destroyed before reporting its case, including a failing case. These are
ordinary source operations using the existing Test capability, without shrinking
or a new runtime assertion. The resource-valued conformance example is
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
```

Update borrows bytes and accepts an empty chunk. Before changing state it checks
that total input remains at most 2^61-1 bytes, the maximum whole-byte message
whose bit length fits SHA-256's 64-bit length field; exceeding it fails SEM-1.
Finish consumes state, pads once, and emits exactly 32 bytes in big-endian word
order. Round additions wrap deliberately at 32 bits. `digest` is the one-shot
form over the same kernel. Hexadecimal rendering is a separate codec. This is a
digest, without password-hashing or signature operations.

## ASCII radix digits

`ascii.digit_value_radix(cp: u32, radix: u8) -> Option[u32] uses {}` accepts
radices 2 through 36 and decimal ASCII digits plus both ASCII letter cases.
Values outside the radix, invalid radices, and non-ASCII scalars return None.
It shares the decimal `digit_value` and ASCII case kernel; the existing decimal
helper retains its signature. `ascii.is_space` remains the sole ASCII-whitespace
predicate (space and HT/LF/VT/FF/CR); split_ascii_whitespace retains its documented
exclusion of VT. Hex/JSON/URI/network parsing and string ASCII helpers reuse these
scalar kernels rather than retaining private digit/classification copies.

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

`std@1/utf8.append_scalar(mut out: List[u8], scalar: u32) -> Result[(),
utf8.EncodeError] uses {}` appends the scalar's UTF-8 encoding. `EncodeError =
InvalidScalar(u32)` rejects surrogates and values above U+10FFFF before touching
`out`. Existing bytes in the builder are preserved on this validation error.
`utf8.decode_lossy(data: Bytes) -> unique String uses {}` borrows arbitrary bytes and
returns owned UTF-8. It replaces each invalid sequence with U+FFFD using Rust's
maximal valid-prefix/error-span policy: a valid but incomplete prefix is replaced
once; an invalid next byte is reconsidered as the start of the next sequence.
Overlong encodings, surrogate encodings and values above U+10FFFF are invalid.
Both operations are ordinary pure source. Strict conversion remains the existing
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
language or native regex hook. An unescaped `{` always begins a repetition;
malformed repetitions are errors. Literal braces can be escaped.

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
fn glob.matches(pattern: glob.Pattern, path: String) -> bool uses {}
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
