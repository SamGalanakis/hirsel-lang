# Standard library

The standard library is the ordinary source package `std@1`, at
`stdlib/std@1/hirsel.json`. A program selects it like any other dependency
([packages](programs.md#package-selection)); it has no core aliases, no builtin bodies and no
privileged hooks, and it uses no `Unsafe`. It raises language failures only through
`failure.check` ([core library](core-library.md#memory-and-iteration)). Its algorithms
follow named reference implementations, ported with their tests.

## The catalogue

These declarations are normative: the source package must declare exactly these
signatures. `N` keeps the ledger's meaning of an explicit `@noalloc` promise.

```text
fn std@1/path.normalize(p: Path) -> Result[Path, std@1/path.PathError] uses {}
fn std@1/path.join(base: Path, child: Path) -> Result[Path, std@1/path.PathError] uses {}
fn std@1/path.parent(p: Path) -> Option[Path] uses {}
fn std@1/path.filename(p: Path) -> String uses {}
fn std@1/path.extension(p: Path) -> String uses {}
fn std@1/path.components(p: Path) -> List[String] uses {}
N fn std@1/duration.from_parts(seconds: i64, nanoseconds: i32) -> std@1/time.Duration uses {}
N fn std@1/duration.parse(text: String) -> Result[std@1/time.Duration, std@1/scalar.ParseError] uses {}
fn std@1/duration.format(value: std@1/time.Duration) -> String uses {}
N fn std@1/time.parse(text: String) -> Result[std@1/time.Timestamp, std@1/scalar.ParseError] uses {}
fn std@1/time.format(value: std@1/time.Timestamp) -> Result[String, std@1/time.TimeError] uses {}
N fn std@1/time.elapsed(clock: MonotonicClock, start: Instant of clock) -> Result[Duration, ClockError]
N fn std@1/time.between(clock: MonotonicClock, start: Instant of clock, end: Instant of clock) -> Duration
N fn std@1/time.compare(clock: MonotonicClock, a: Instant of clock, b: Instant of clock) -> i32
fn std@1/bytes.replace(src: Bytes, old: Bytes, new: Bytes) -> Bytes in src
N fn std@1/map.same_entries[K: key, V: data](a: Map[K, V], b: Map[K, V]) -> bool
N fn std@1/unicode.graphemes(s: String) -> GraphemeView in s
fn std@1/json.parse(take text: String) -> Result[Value, JsonError] uses {}
fn std@1/json.stringify(value: Value) -> Result[String, JsonError] uses {}
fn std@1/json.write(value: Value, mut builder: List[u8]) -> Result[(), JsonError] uses {}
N fn std@1/test.equal[T: data](t: Test, actual: T, expected: T) -> ()
N fn std@1/test.approx(t: Test, actual: f64, expected: f64, absolute: f64, relative: f64) -> ()
fn std@1/test.for_all[T](t: Test, gen: fn(Test) -> T, law: fn(T) -> bool uses {}) -> ()
fn std@1/test.expect_error[T, E](t: Test, take value: Result[T, E]) -> E
fn std@1/input.read_all(s: Stdin) -> Result[Bytes, InputError]
fn std@1/input.read_line(s: Stdin) -> Result[Option[String], InputError]
```

Modules beyond the catalogue (`list`, `string`, `text`, `net`, `uri`, `http_wire`, `http`, `bufio`
and the `format_into` and NFC functions below) define their contracts by their declared
signatures and the rules in this part.

## Paths

`std@1/path.PathError = Invalid(u64)` reports a byte offset. Components are separated by
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

`std@1/time.Duration` holds signed `i128` nanoseconds. `duration.from_parts` widens signed
`i64` seconds and signed `i32` nanoseconds before summing, with no range imposed on the
fractional part. `duration.parse` accepts an optional sign, integer decimal seconds, an
optional dot followed by one to nine fraction digits, and a final `s`, with no whitespace.
Invalid grammar is `std@1/scalar.ParseError.Invalid`; a value not representable as signed
`i128` nanoseconds is `OutOfRange`. `duration.format` covers the whole signed `i128` range
and emits canonical seconds with a final `s` and trimmed fractional zeros; zero is `0s`.

`std@1/time.Timestamp` holds signed `i128` `unix_nanoseconds`; `TimeError` is `OutOfRange`.
`time.parse` accepts RFC 3339 with Gregorian years 0000–9999, a `T` or `t` separator, a `Z`,
`z` or numeric `±HH:MM` offset (hours 00–23, minutes 00–59), and an optional fraction of one
to nine digits, validating dates and 00–23/00–59/00–59 times. Space separators, a leap
second 60, a missing offset and trailing data are invalid. Numeric offsets normalize to UTC,
`-00:00` included; an offset may move the UTC result outside the formatting range.
`time.format` returns UTC with a four-digit year, uppercase `T` and `Z` and trimmed
fractional zeros, or `TimeError.OutOfRange` outside years 0000–9999. These are ordinary
algorithms with no clock authority, locale or time-zone database.

`time.elapsed`, `time.between` and `time.compare` work on `Instant`s of one
`MonotonicClock`; their brand ties both instants to that clock.

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

## Scalars

`bool`, `u8` to `u128`, `i8` to `i128`, `f32` and `f64` each have a module exposing, with
`uses {}`:

- `to_string(value) -> String`;
- `format_into(value, mut out: List[u8]) -> ()`, which appends exactly the text `to_string`
  returns and fails only by allocation exhaustion;
- `@noalloc format[n: size](value, mut buffer: Inline[u8, n]) -> Result[u64, std@1/scalar.FormatError]`,
  which checks capacity before writing, returns the length of the UTF-8 prefix and leaves
  the whole buffer unchanged on error;
- `@noalloc parse(text: String) -> Result[scalar, std@1/scalar.ParseError]`.

`FormatError` is `BufferTooSmall()`; `ParseError` is `Invalid | OutOfRange`. `bool` is
exactly `true` or `false`. Integers use decimal `-?[0-9]+`, with a minus only for signed
types; leading zeros are accepted; a plus sign, whitespace, underscores and radix prefixes
are rejected; unrepresentable values are `OutOfRange`.

Floats format with the shortest decimal digits that round-trip, ties to even. Fixed
notation is used for decimal exponents in [-6, 21), and `d[.ddd]e[-]N` otherwise (the
ECMAScript `Number::toString` thresholds). Whole fixed numbers omit `.0`, negative zero is
`-0`, and the nonfinites are `NaN`, `inf` and `-inf`. Float parsing accepts
`-?[0-9]+(\.[0-9]+)?([eE][-+]?[0-9]+)?` and exactly those nonfinite spellings, and rounds
correctly, ties to even. `OutOfRange` applies exactly when correct IEEE rounding yields
infinity; values above `MAX` that round back to `MAX` succeed. Underflow rounds to a
subnormal or signed zero without error.

`@noalloc f32.sum(xs: Array[f32]) -> f32` and `f64.sum(xs: Array[f64]) -> f64`, and
`f32.sum_exact` and `f64.sum_exact` with the same types and allocation allowed, implement the
reductions of [the core library](core-library.md#numeric-and-string-details).

## Testing helpers

`test.equal` checks structural equality of `data` values. `test.approx` requires
nonnegative tolerances, accepts exactly equal values, rejects unequal nonfinite values, and
otherwise passes when the difference is within the absolute tolerance or within the
relative tolerance of the larger magnitude. `test.for_all` runs exactly 100 sequential
generator and law calls using the run's Test grant, reporting each through
`test.check_case`, without shrinking. `test.expect_error` returns the error of an `Error`,
and destroys an unexpected `Ok` value before failing the test with `test.fail`.

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
`split_ascii_whitespace(take s: String) -> List[String]` (on ASCII tab, LF,
form feed, CR and space; omitting empty fields and preserving vertical tab and
non-ASCII whitespace), `starts_with`, `ends_with`, `trim` (ASCII space and `\t` to `\r`, a slice related to the
source), `ascii_lower`, `ascii_upper`, `concat`, `join` and `compare(a, b) -> i32`
(byte-lexicographic, a proper prefix first, as `<` orders `String`; -1, 0 or 1, so
it is a `sort` comparator). `std@1/text` treats a
`List[u8]` as a UTF-8 builder: `append` appends a string's bytes and `finish` validates the
builder into a `String` or returns the bytes.

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
fn std@1/bufio.new(capacity: u64) -> Reader uses {}
fn std@1/bufio.buffered(r: Reader) -> u64 uses {}
fn std@1/bufio.contents(r: Reader) -> Bytes uses {}
fn std@1/bufio.consume(mut r: Reader, count: u64) -> () uses {}
fn std@1/bufio.read_more[S,E](mut r: Reader, mut source: S, refill: fn(mut S, mut List[u8], u64) -> Result[u64,E], max: u64) -> Result[u64,ReadError[E]]
fn std@1/bufio.read[S,E](mut r: Reader, mut source: S, refill: fn(mut S, mut List[u8], u64) -> Result[u64,E], mut out: List[u8], max: u64) -> Result[u64,ReadError[E]]
fn std@1/bufio.read_exact[S,E](mut r: Reader, mut source: S, refill: fn(mut S, mut List[u8], u64) -> Result[u64,E], mut out: List[u8], count: u64) -> Result[(),ReadError[E]]
fn std@1/bufio.read_until[S,E](mut r: Reader, mut source: S, refill: fn(mut S, mut List[u8], u64) -> Result[u64,E], delimiter: u8, mut out: List[u8], limit: u64) -> Result[u64,ReadError[E]]
fn std@1/bufio.read_line[S,E](mut r: Reader, mut source: S, refill: fn(mut S, mut List[u8], u64) -> Result[u64,E], limit: u64) -> Result[String,ReadError[E]]
fn std@1/bufio.stream_read_more(n: Net, mut s: Stream of n, mut r: Reader, max: u64) -> Result[u64,ReadError[NetError]]
fn std@1/bufio.stream_read(n: Net, mut s: Stream of n, mut r: Reader, mut out: List[u8], max: u64) -> Result[u64,ReadError[NetError]]
fn std@1/bufio.stream_read_exact(n: Net, mut s: Stream of n, mut r: Reader, mut out: List[u8], count: u64) -> Result[(),ReadError[NetError]]
fn std@1/bufio.stream_read_until(n: Net, mut s: Stream of n, mut r: Reader, delimiter: u8, mut out: List[u8], limit: u64) -> Result[u64,ReadError[NetError]]
fn std@1/bufio.stream_read_line(n: Net, mut s: Stream of n, mut r: Reader, limit: u64) -> Result[String,ReadError[NetError]]
fn std@1/bufio.file_read_more(d: Dir, mut f: File of d, mut r: Reader, max: u64) -> Result[u64,ReadError[FileError]]
fn std@1/bufio.file_read(d: Dir, mut f: File of d, mut r: Reader, mut out: List[u8], max: u64) -> Result[u64,ReadError[FileError]]
fn std@1/bufio.file_read_exact(d: Dir, mut f: File of d, mut r: Reader, mut out: List[u8], count: u64) -> Result[(),ReadError[FileError]]
fn std@1/bufio.file_read_until(d: Dir, mut f: File of d, mut r: Reader, delimiter: u8, mut out: List[u8], limit: u64) -> Result[u64,ReadError[FileError]]
fn std@1/bufio.file_read_line(d: Dir, mut f: File of d, mut r: Reader, limit: u64) -> Result[String,ReadError[FileError]]
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
fn line(n: Net, mut stream: Stream of n) -> Result[String, std@1/bufio.ReadError[NetError]] {
  let mut reader = std@1/bufio.new(4096)
  std@1/bufio.stream_read_line(n, mut stream, mut reader, 8192)
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
HttpError    = Net(NetError) | Url(std@1/uri.UriError) | Malformed(std@1/http_wire.HeadError)
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

`std@1/input.read_all(s: Stdin) -> Result[Bytes, InputError]` reads to end of input.
`std@1/input.read_line(s: Stdin) -> Result[Option[String], InputError]` reads through
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
N fn std@1/http_wire.parse_request[k: size](buf: Bytes, mut line: RequestLine, mut headers: Inline[RawHeader, k], mut count: u64) -> Result[ParseStatus, HeadError] uses {}
ServerRequest = { method: String, target: String, version: Version, headers: List[Header], body: Bytes }
fn std@1/http.parse_server_request(buf: Bytes, max_head_bytes: u64, max_body_bytes: u64) -> Result[Option[(ServerRequest, u64)], HttpError] uses {}
fn std@1/http.receive_request(n: Net, mut s: Stream of n, max_head_bytes: u64, max_body_bytes: u64) -> Result[ServerRequest, HttpError]
fn std@1/http.respond(n: Net, mut s: Stream of n, status: u16, headers: List[Header], body: Bytes) -> Result[(), HttpError]
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
