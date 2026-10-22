# csvjoin

A bounded CSV inner/left join using the shipped `std@1/csv` reader and writer.

```sh
hirsel run examples/csvjoin/hirsel.json --grants grants.json -- \
  left.csv right.csv --left-key id --right-key customer_id --mode left
```

Grant `io.stdout` (`stdout.bytes@1`) and a read-only directory at virtual
`data`; both paths are relative to that directory. `--stats` additionally
requires `io.stderr` (`stderr.bytes@1`). The same application arguments work immediately after the grants file with binaries
built using `hirsel build --backend llvm|cranelift` and `--hirsel-grants`.

Both inputs must have a header record, unique names (including at most one
empty name), and the named key. Empty files fail; header-only files succeed.
Every data record must have the header's width. Keys are exact UTF-8 strings:
`01`, `1`, whitespace and empty keys are distinct, with empty matching empty.

Output is all left columns followed by right columns except the right key.
Right names that collide with any output name repeatedly gain `right_` until
unique, in right column order. For example left `id,name,right_name` and right
`key,name,right_name` produce `id,name,right_name,right_right_name,right_right_right_name`.
Rows follow left input order; each left row emits matching right rows in right
input order (the Cartesian product for duplicates). Inner mode omits unmatched
rows; left mode appends empty right fields. There is no map iteration in output.

CSV parsing, quoting and UTF-8 validation use std, never comma splitting.
The std dialect ignores blank physical lines, normalizes CRLF (including inside
quoted fields) to LF, and writes LF records. Quoted commas, doubled quotes,
embedded newlines and Unicode are supported. A sole empty output field is
written as `""` to preserve it through the reader’s blank-line skipping. Returned errors identify side
`left`/`right`, 1-based logical record (header is 1), and the exact 0-based raw
byte cursor just after the offending byte, or EOF for unfinished records.
Syntax errors additionally carry std physical line and byte column.

All budgets are finite; flags may lower these hard ceilings (zero is allowed):

| Flag | Default / ceiling | Accounting |
| --- | ---: | --- |
| `--max-right-bytes` | 16777216 | raw right input bytes including header |
| `--max-right-rows` | 100000 | indexed right data records |
| `--max-key-bytes` | 4096 | UTF-8 bytes in either key |
| `--max-row-bytes` | 1048576 | raw bytes per input record, including terminator and skipped blank lines |
| `--max-output-rows` | 1000000 | output data records, excluding header |
| `--max-output-bytes` | 268435456 | encoded bytes including header and LF |

The right index is fully validated before any output. The left header is then
validated and the output header emitted. Errors exit 1; successful joins exit 0.
Output is streaming: a later left error or output cap can leave a valid prefix.
The entire match count is checked against the row budget before expanding a
left record; encoded byte limits are checked before each write. A temporary
encoded output record is bounded by two input records, independently of the
output budget. Budgets bound retained data, not an exact allocator/RSS total:
container/string metadata and map capacity add overhead. The byte/row/key limits
together bound index memory. Ordinary join working memory on the left depends on header/row size and a 4096
byte file buffer, not total left file size. `--stats` reports deterministic
counts and the maximum observed raw left record size; these are streaming
controls, not a claim of a performance improvement. Recording separately
stores file snapshots for replay; tape storage and the recording infrastructure
can therefore scale with total file size. Replay does not require the originals.

The adapter supplies one byte at a time from the buffered file to the std CSV
reader so diagnostics and raw-byte budgets need no parser-internal API. File
reads remain buffered. It retains only the current left record and one encoded
output record; no full input or duplicate expansion is materialized.

Prior art: BurntSushi/rust-csv `05612e87e6e92910d40d7214b70952b8434fef9b`,
`examples/tutorial-perf-alloc-02.rs:1–26` supplies the streaming record loop;
`tests/tests.rs:1–65` and `examples/data/smallpop.csv` supply cookbook vectors.
The existing `examples/tabfold/main.hir:1–61` at
`7d84d688c5eb788b4c0b0dae20cf47dbf87acb8f` supplies CLI/grant integration.
Adaptations: std owns each decoded record instead of byte-record borrowing;
Hirsel capabilities replace stdin; unique owned index copies allow bucket removal; an ordered duplicate index and finite budgets
implement the join; buffered single-byte delivery supplies exact diagnostics;
a sole empty output field is quoted to round-trip through the shipped reader.
No CSV parser is ported. The independent Python `csv` oracle is in
`crates/hirsel-cli/tests/support/csvjoin_15/oracle.py`.

The rust-csv structure/vectors are used under its MIT option:

Copyright (c) 2015 Andrew Gallant

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
