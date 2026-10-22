# Canonical HTTP dates

`std@1/http_date.parse(String)` returns `Result[time.Timestamp, http_date.ParseError]`.
`std@1/http_date.format(time.Timestamp)` returns `Result[unique String, http_date.FormatError]`.
Both use `uses {}`: there is no clock, timezone or locale capability.

This is a strict canonical IMF-fixdate subset, **not the full permissive HTTP
recipient date parser**. The only accepted spelling is exactly 29 ASCII bytes:
`Sun, 06 Nov 1994 08:49:37 GMT`. English weekday/month names, case, punctuation,
spaces and `GMT` must match. The weekday must agree with the Gregorian date.
Years are 1970–9999, hours 00–23, minutes/seconds 00–59. Invalid calendar dates,
leap seconds, fractions, other zones, surrounding whitespace, trailing data,
RFC850 and asctime forms are rejected. Parsing does not trim or normalize.

`ParseError` carries a zero-based **byte** `offset` and `kind`:

| Kind | Offset |
| --- | --- |
| Length | first missing byte, or 29 for excess input |
| NonAscii | first non-ASCII byte encountered during structural validation |
| Separator | incorrect punctuation, space or GMT byte |
| Weekday | 0 (invalid token or mismatched calendar weekday) |
| Month | 8 |
| Digit | offending numeric byte |
| Year | 12 |
| Day | 5 (including an impossible day for that month/year) |
| Hour / Minute / Second | 17 / 20 / 23 |
| Calendar | 17 for time construction, 12 for timestamp conversion |

Length is checked first; structural validation then scans left to right, followed
by tokens, numeric fields, ranges and weekday agreement. For multiple defects,
the error follows that validation order. Calendar conversion failures remain typed.

Formatting uses UTC and rejects any timestamp outside 1970-01-01 through the end
of 9999-12-31 as `OutOfRange()`, including extreme i128 values. Within that range,
a nonzero nanosecond fraction is `Subsecond()`. It never rounds or silently
normalizes; the last accepted timestamp is 253402300799000000000 nanoseconds.

Applications explicitly construct ordinary header fields:

```hirsel
let stamp = time.Timestamp(unix_nanoseconds: 784111777000000000i128)
let Ok(date_text) = http_date.format(stamp) else { return Error(()) }
let date_header = http.Header(name: "Date", value: take date_text)
let Ok(modified_text) = http_date.format(stamp) else { return Error(()) }
let conditional = http.Header(name: "If-Modified-Since", value: take modified_text)
```

For an incoming `If-Modified-Since` field, pass the field value to
`http_date.parse`; the application decides how to handle a typed parse error.
This adapter does not insert headers or change HTTP parsing/serialization.

## Prior art and deviations

Ported from [pyfisch/httpdate](https://github.com/pyfisch/httpdate), commit
`53df52094835dd85aca70e23b9f384abcf843f7f` (MIT): `src/date.rs:33–44`
(validated fields), `180–228` (fixed-width formatter and token tables), `277–316`
(IMF parser); compatible epoch/calendar vectors from `src/lib.rs:71–117`.
The reference checkout pin was verified before porting.

Deviations: no trimming or obsolete-form fallback (strict subset contract);
typed byte errors replace the opaque error; formatting rejects fractions and
unsupported ranges instead of truncation/panic. Hirsel lists build the fixed
text instead of mutating a Rust byte array; scalar tuples keep three-byte
tokens inline so parsing satisfies Hirsel’s `@noalloc` guarantee. Calendar validation, weekday and
UTC epoch conversion use existing `date`, `time_of_day` and `date_time` APIs;
none of the reference civil arithmetic or SystemTime/clock access is ported.
Existing calendar modules and HTTP handling remain unchanged.

The tests assert fixed text/epoch pairs before roundtrips, cover every month and
weekday, leap year 2000 and common year 2100, day/month/year rollover, strict
errors and epoch/year-9999/i128 boundaries. Additional 2024 month spellings and
epoch values were checked with Python's independent UTC Gregorian calendar.
The full 33-vector calendar fixture runs in a bounded 5M-step Rust harness on
H, LLVM and Cranelift; the corpus keeps eight boundary vectors under its 1M cap.

## Upstream MIT notice

```text
Copyright (c) 2016 Pyfisch

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in
all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN
THE SOFTWARE.
```
