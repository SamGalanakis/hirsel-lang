# Binary line scanning

Port of Go `bufio.ScanLines`, `src/bufio/scan.go:139–183,345–373`, and
`src/bufio/scan_test.go:175–212,267–308` at
`ebe2feefc400f813aefdc8a036e66d2619da6649`.

## API and state

`std@1/line_scan.new(max_line_bytes, strip_cr)` returns
`Result[unique Scanner, Error]`. Zero is `InvalidPolicy`; any nonzero `u64`
is a valid raw-byte limit. Mutating operations require a unique scanner. Its opaque surface promises
`inert + noalloc_drop`, matching the concrete builder/status representation.
`feed(mut scanner, chunk: Bytes, eof: bool)` borrows its input and returns
`Result[List[Bytes], Error]` whose byte storage is owned independently of
both input and scanner. Retaining results never forces later carry copies.
`status`, `offset`, and `buffered` inspect state without changing it.

| State | Input | Result and next state |
| --- | --- | --- |
| Open | Empty, nonfinal | Empty output; Open |
| Open | Bounded bytes, nonfinal | Complete lines; Open |
| Open | Bounded bytes, EOF | Complete lines and a nonempty tail; Finished |
| Open | First over-limit raw byte | `LineTooLong(offset)`; clear carry; Poisoned |
| Open | Absolute offset overflow | `OffsetOverflow`; clear carry; Poisoned |
| Finished | Any feed, including empty EOF | `AlreadyFinished`; Finished |
| Poisoned | Any feed | `ScannerFailed`; Poisoned |

LF separates lines and is excluded. The limit counts bytes before LF,
including CR; stripping cannot make an oversized raw line valid. With
`strip_cr = true`, remove exactly one terminal CR from each emitted line,
including an unterminated EOF tail. Bare CR elsewhere is ordinary binary
data. A terminal LF does not add another line. A nonempty raw EOF tail is
emitted even when stripping its lone CR produces an empty line. NUL and
invalid UTF-8 have no special meaning here.

`offset` counts all bytes accepted by successful feeds, including LF. On
`LineTooLong(n)` it names the zero-based absolute offset of the first byte
beyond the limit; that byte is rejected. On offset overflow it stays at the
previous successful offset. Both failures discard **every line built by the
failing call**, including lines completed earlier in that chunk. Results of
previous calls stay valid. A poisoned scanner cannot resume or retry.

## Work and memory

Delimiter scanning reads each newly supplied byte once; it never scans old
carry from its beginning. Check segment length with subtraction before
appending, then append each unmatched segment once. Finished lines move the
carry builder into independent Bytes. One trailing-byte inspection and pop
implements optional CR removal. This costs O(input bytes + emitted lines),
with amortized builder growth; repeated one-byte feeds have linear work.

Retained logical carry is at most `max_line_bytes` raw bytes. Its backing
allocation follows the ordinary List builder's geometric capacity policy,
so allocated capacity is O(limit), rather than a promise of an exact byte
allocation cap. A failed feed releases carry. Input chunks belong to the
caller and are not retained. **Output is separately bounded by the current
feed's data plus previous carry**, and by the number of LF delimiters: one
large chunk with many blank lines can allocate a large List of results.
The API does not promise O(limit) total memory for a whole feed. Choose
bounded chunks and consume/drop each returned batch to bound whole-program
memory.

## NDJSON-lines example

`examples/ndjson-lines` reads stdin in chunks of at most 4096 bytes, with a
1 MiB raw line limit and CR stripping. It skips only empty physical lines
(after CR stripping); whitespace-only lines are parsed and can fail. Every
other line is validated as UTF-8 through `string.from_bytes`, then passed
to existing `json.parse` and `json.stringify` APIs. Output is one compact
JSON value per line. The example accepts any JSON value under those APIs'
existing syntax, duplicate-key and nesting policies; it makes no broader
NDJSON standard claim.

Malformed UTF-8 or JSON stops at the first offending line, prints
`line N: invalid UTF-8` or `line N: invalid JSON` to stderr, and returns
`InvalidUtf8(N)` or `InvalidJson(N, JsonError)`. N is one-based and counts
blank lines. Valid earlier values may already have been printed. No malformed
line is silently skipped. Scanner and I/O errors propagate as distinct arms.

Run with explicit grants (using a built Hirsel CLI):

```sh
printf '\n{"x":1}\r\n[true,null]\n42' | hirsel run examples/ndjson-lines/hirsel.json --grants '{"io":{"stdin":"stdin.bytes@1","stdout":"stdout.bytes@1","stderr":"stderr.bytes@1","dirs":[]}}'
```

Expected stdout is `{"x":1}\n[true,null]\n42\n` and stderr is empty.

## Tests and reference adaptations

The lane owns `conformance/SEM/SEM-3-line-scan-7-{controls,vectors}-run.hir` and their
manifests, the bounded helper fixture
`crates/hirsel-driver/tests/fixtures/line-scan-7/exhaustive.hir` and manifest,
and `crates/hirsel-driver/tests/it-stdlib/line_scan_7.rs`. The corpus stays
within its 1M-step budget; exhaustive and rising/falling line workloads live
in a bounded Rust harness. Short cases cover every partition, every pair of
cuts (including empty chunks), and both CR policies. A whole-input Rust
splitter supplies independent expected raw-limit offsets and lines. Go's
final-tail vectors and increasing/decreasing line lengths are retained.

H heap campaigns inject allocation failures and check ownership/cleanup.
H byte-read counts for 64/128/256 one-byte feeds exclude prefix rescanning;
zero carry duplication excludes hidden copy-on-write prefix copies. Both
native backends execute vectors and the example's fixed stdin cases.

Deviations from Go: caller-fed bytes replace reader refills; an explicit
positive raw-line bound excludes LF but includes CR, rather than Go's token
buffer bound (which includes delimiter space); CR removal is optional;
owned results replace recycled token views; errors are structured and poison
state explicitly; a failing call publishes no partial batch. These changes
serve the feed API and Hirsel ownership/error semantics. Empty nonfinal
feeds need no Go-style empty-read panic because the caller controls progress.
No language, TIR/LIR, runtime ABI, bufio, stream or JSON changes are needed.

## Reference license

```text
Copyright 2009 The Go Authors.

Redistribution and use in source and binary forms, with or without
modification, are permitted provided that the following conditions are
met:

   * Redistributions of source code must retain the above copyright
notice, this list of conditions and the following disclaimer.
   * Redistributions in binary form must reproduce the above
copyright notice, this list of conditions and the following disclaimer
in the documentation and/or other materials provided with the
distribution.
   * Neither the name of Google LLC nor the names of its
contributors may be used to endorse or promote products derived from
this software without specific prior written permission.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS
"AS IS" AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT
LIMITED TO, THE IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR
A PARTICULAR PURPOSE ARE DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT
OWNER OR CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL,
SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT
LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES; LOSS OF USE,
DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON ANY
THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT
(INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE
OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.
```
