# csvreduce

Aggregate one integer column of a CSV file with fixed contiguous chunks and
ordered reduction. The first CSV record is a header. Column indices start at zero.
The header's width applies to every data record; names need not be unique.

Build the matching CLI and standard-library image from this checkout:

```sh
python3 tools/buck2/prepare.py
python3 tools/buck2/sync.py --if-stale
.buck2/bin/buck2 build //:hirsel-cli-bin-hirsel //:hirsel-std-image //tools/buck2/native-toolchain:llvm //tools/buck2/native-toolchain:kit //:hirsel-rt-archives --show-output
```

Set `HIRSEL` to the printed CLI path, `HIRSEL_IMAGE` to the printed image path,
`LLVM_SYS_231_PREFIX` to the printed LLVM directory, `HIRSEL_LINK_KIT` to the
printed kit directory, and `HIRSEL_RUNTIME_ARCHIVES` to the runtime archives
directory. Instantiate
`grants.json`: replace `/ABSOLUTE/CSV/DIRECTORY` with the absolute directory
containing the input file. The tool reads relative to that read-only `data` mount.
It prints exactly one JSON object to stdout and creates no output file.

```sh
HIRSEL_JOBS=1 tools/capped --timeout 60 "$HIRSEL" run --native --backend llvm \
  examples/csvreduce/hirsel.json --grants /absolute/grants.json --cores 2 \
  --record /absolute/csvreduce.tape -- input.csv 1 --grain 3
```

For `fixtures/mixed.csv` the output is:

```json
{"count":5,"sum":7,"min":-9,"max":12}
```

Use `--backend cranelift` for the other native backend. Omit `--native` and
`--backend` for the interpreter. `HIRSEL_JOBS` limits compiler jobs; production
Ordered width comes from `--cores` (or grants placement). The production provider
is `tasks.cooperative@1` with the default FIFO schedule. Remove `HIRSEL_RT_*`
test overrides when recording production runs. Shared commit and auto reorder
are explicit in the template. Optional placement `metrics` and `control` paths
are host diagnostics outside semantic identity.

Replay needs the original source revision and immutable tape, and reads no input
file or live capability provider:

```sh
HIRSEL_JOBS=1 tools/capped --timeout 60 "$HIRSEL" run \
  examples/csvreduce/hirsel.json --replay /absolute/csvreduce.tape
```

Admission bounds are 262144 input bytes, 4096 data records, 32 columns, signed i64
cells in [-1000000000, 1000000000], and at most 16 chunks. `--grain ROWS` accepts
1..4096 rows per chunk; `ceil(data_records / ROWS)` must be at most 16. A grain of
1 therefore accepts at most 16 data rows. For 4096 rows use at least 256. Empty
data has count/sum zero and null min/max. No header, invalid UTF-8/CSV/integer,
ragged rows, invalid column, exceeded bound, or bad arguments return an error and
exit 1 with no summary. Signed decimal parsing follows `i64.parse`; whitespace
is not trimmed. Bounds guarantee count <=4096 and abs(sum) <=4096000000000, so
both are exact and fit their integer types without overflow.

The bounded file reader probes one extra byte at the byte limit. Std CSV parses
complete records before partitioning; quoted multiline fields, quoted numbers,
CRLF and missing final newlines retain std policy. Rows stay immutable under one
`Shared` root. Current return ownership requires one explicit deep `unique` copy
of the parsed table before freezing. Chunk descriptors copy only that root and
scalar indices. Each pure `uses {}` callback scans its own rows, producing a
copyable inert summary. `task.map_all(..., 1, compute)` creates one child per
chunk descriptor, joins or cancels every child, and returns input order. Reduction
uses that order. Physical widths never change the partition or grain. There is
no live-worker argument to `map_all`. Pure callbacks cannot be interrupted by a
timeout; the fixed file/row/chunk caps bound each callback's work.

Prior art: Rayon `ee0a00bdb1ab039e178a215ad5712fb7fa58e58f`,
`src/iter/fold.rs:35–95` and `src/iter/test.rs:78–128` (MIT/Apache-2.0), provides
independent fold identities and sequential equivalence for the 0..1023 map +1
vector. This tool materializes those mapped cells as CSV input and uses fixed
contiguous partitions and input-order reduction instead of Rayon's dynamic
splits. Go `ebe2feefc400f813aefdc8a036e66d2619da6649`,
`src/encoding/csv/reader.go:197–205` and `reader_test.go:50–100` (BSD-3-Clause),
provides complete-record and multiline/CRLF/NoEOL fixture shapes. Tests strip
Go's §/¶ position markers and add numeric cells. CSV policy remains the landed
`csv-upstream-25` policy: explicit width checking, terminal errors, UTF-8 fields,
no recovery/partial rows or Go reader options. The implementation calls std CSV;
it contains no ported parser. See [third-party notices](../../third-party/NOTICES.md).

Focused tests: `csvreduce_27` in `hirsel-cli-test-cli-shard-1` with
`-c hirsel.tests=focus`. Six small partitions execute file-read/parse/map/reduce
in interpreter, checked H, heap, LLVM and Cranelift. A separate CLI test exercises
the shipped argument boundary and replay with the original host directory absent.
Native production placement and timing evidence are recorded externally; passing
semantic engines alone does not establish effective physical width or speedup.

Production canonical trace export currently has an owner-routed Args observer
defect (`e1-trace-record-27`); its permanent regression is listed in
`tools/known-failures.tsv`. Ordinary recording and immutable replay passed at
live-observed widths 1/2/4 in both backends. Trace-export qualification and
certified wall-time scaling remain separate incomplete cells until repaired.
